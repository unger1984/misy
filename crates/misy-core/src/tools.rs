//! Local agent tools: their provider-visible definitions, JSON Schema validation of
//! arguments, and asynchronous execution of the built-ins. The dispatcher's definition list is
//! the single source of truth for the tools declared to the model, and per-tool budgets (read
//! and listing caps, command timeout) keep one call from growing a response without bound.
use crate::{ActivityId, ActivityOutput, ActivitySummary, ToolCall, ToolDefinition, ToolResult};
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, fs, io::Read};
use tokio::task::spawn_blocking;

mod activity;
mod command;
mod image_view;
mod stdin;
#[cfg(test)]
mod tests;

pub(crate) use activity::ActivityEvent;

/// Upper bound for one `read_file` result; larger files are truncated with a marker.
const MAX_READ_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Upper bound for one `list_directory` result; extra entries are summarized in a marker.
const MAX_DIRECTORY_ENTRIES: usize = 10_000;

/// Definitions available to providers. Built-ins are installed on construction.
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, RegisteredTool>,
}

#[derive(Clone, Debug)]
struct RegisteredTool {
    definition: ToolDefinition,
    validator: Validator,
}

impl ToolRegistry {
    /// Creates a registry containing all built-in tool definitions.
    ///
    /// # Panics
    ///
    /// Panics only if a statically defined built-in schema is invalid, which is a build-time
    /// invariant covered by tests.
    pub fn new() -> Self {
        let definitions = builtin_definitions()
            .into_iter()
            .map(|definition| {
                let validator = compile_schema(&definition.input_schema)
                    .expect("built-in tool schemas must be valid JSON Schema draft 2020-12");
                (
                    definition.name.clone(),
                    RegisteredTool {
                        definition,
                        validator,
                    },
                )
            })
            .collect();
        Self { definitions }
    }

    /// Returns cloned definitions for every registered tool in stable name order.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    /// Looks up a tool definition by provider-visible name.
    #[cfg(feature = "test-support")]
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name).map(|tool| &tool.definition)
    }

    /// Registers a new tool after compiling its JSON Schema.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate names or invalid schemas.
    #[cfg(feature = "test-support")]
    pub fn register(&mut self, definition: ToolDefinition) -> Result<(), ToolRegistryError> {
        if self.definitions.contains_key(&definition.name) {
            return Err(ToolRegistryError::DuplicateTool(definition.name));
        }
        let validator = compile_schema(&definition.input_schema)?;
        self.definitions.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                validator,
            },
        );
        Ok(())
    }

    /// Validates one provider-supplied argument value against a registered tool schema.
    ///
    /// # Errors
    ///
    /// Returns an error when the tool is unknown or its arguments do not satisfy the schema.
    pub fn validate_arguments(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<(), ToolRegistryError> {
        let tool = self
            .definitions
            .get(name)
            .ok_or_else(|| ToolRegistryError::UnknownTool(name.to_owned()))?;
        tool.validator
            .validate(arguments)
            .map_err(|error| ToolRegistryError::InvalidArguments(error.to_string()))
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors while defining or validating local tools.
#[derive(Debug, Eq, PartialEq)]
pub enum ToolRegistryError {
    /// A tool with this name has already been registered.
    // Only the test-support `ToolRegistry::register` produces this variant.
    #[cfg(feature = "test-support")]
    DuplicateTool(String),
    /// A JSON Schema could not be compiled.
    InvalidSchema(String),
    /// No tool has this provider-visible name.
    UnknownTool(String),
    /// Arguments failed the registered JSON Schema.
    InvalidArguments(String),
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "test-support")]
            Self::DuplicateTool(name) => write!(formatter, "tool `{name}` is already registered"),
            Self::InvalidSchema(message) => write!(formatter, "invalid JSON Schema: {message}"),
            Self::UnknownTool(name) => write!(formatter, "unknown tool `{name}`"),
            Self::InvalidArguments(message) => formatter.write_str(message),
        }
    }
}

impl Error for ToolRegistryError {}

/// Executes registered built-in tools asynchronously; callers retain source-order dispatch.
#[derive(Clone, Debug)]
pub struct ToolDispatcher {
    registry: ToolRegistry,
    activities: activity::ActivityManager,
    command_timeout: Option<std::time::Duration>,
}

impl ToolDispatcher {
    /// Creates an asynchronous dispatcher backed by `registry`.
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry,
            activities: activity::ActivityManager::new(),
            command_timeout: None,
        }
    }

    /// Returns cloned definitions for every registered tool in stable name order.
    ///
    /// This is the single source of truth for the declarations sent to providers: a tool that the
    /// dispatcher can execute must also be declared to the model, so the agent builds
    /// `chat.start` params from this list rather than from a separate registry.
    #[cfg(feature = "test-support")]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions_for(true)
    }

    /// Returns definitions supported by the selected model's input modalities.
    pub fn definitions_for(&self, supports_images: bool) -> Vec<ToolDefinition> {
        self.registry
            .definitions()
            .into_iter()
            .filter(|definition| supports_images || definition.name != "view_image")
            .collect()
    }

    /// Replaces the command execution timeout used by `exec_command`.
    #[cfg(feature = "test-support")]
    pub fn with_command_limits(self, command_timeout: std::time::Duration) -> Self {
        Self {
            command_timeout: Some(command_timeout),
            ..self
        }
    }

    /// Returns active activities followed by the most recent terminal activities.
    pub fn activities(&self) -> Vec<ActivitySummary> {
        self.activities.activities()
    }

    /// Subscribes to activity lifecycle changes.
    pub(crate) fn subscribe_activities(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<ActivityEvent> {
        self.activities.subscribe()
    }

    /// Returns bounded output for a command activity.
    pub async fn activity_output(
        &self,
        id: ActivityId,
        wait: Option<std::time::Duration>,
    ) -> Option<ActivityOutput> {
        self.activities.output(id, wait).await
    }

    /// Requests termination of one active command activity.
    pub fn stop_activity(&self, id: ActivityId) -> bool {
        self.activities.stop(id)
    }

    /// Stops and reaps every active command activity.
    pub async fn shutdown(&self) {
        self.activities.shutdown().await;
        self.activities.flush_events().await;
    }

    /// Validates and executes one local tool call.
    ///
    /// Failures are encoded in [`ToolResult`] so a provider receives the execution outcome rather
    /// than an out-of-band core error.
    #[cfg(feature = "test-support")]
    pub async fn dispatch(&self, call: &ToolCall) -> ToolResult {
        self.dispatch_inner(call, None).await
    }

    pub(crate) async fn dispatch_cancellable(
        &self,
        call: &ToolCall,
        cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> ToolResult {
        self.dispatch_inner(call, Some(cancellation)).await
    }

    async fn dispatch_inner(
        &self,
        call: &ToolCall,
        cancellation: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> ToolResult {
        if let Err(error) = self
            .registry
            .validate_arguments(&call.name, &call.arguments)
        {
            return ToolResult::error(&call.id, error.to_string());
        }
        match call.name.as_str() {
            "list_directory" => self.list_directory(call).await,
            "read_file" => self.read_file(call).await,
            "view_image" => image_view::execute(call).await,
            "exec_command" => self.exec_command(call, cancellation).await,
            "task_list" => self.task_list(call),
            "task_stop" => self.task_stop(call),
            "write_stdin" => stdin::execute(call, &self.activities, cancellation).await,
            "write_file" => self.write_file(call).await,
            _ => ToolResult::error(&call.id, format!("tool `{}` is not executable", call.name)),
        }
    }

    async fn list_directory(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        let path = path.to_owned();
        let names = match spawn_blocking({
            let path = path.clone();
            move || list_directory_entries(&path)
        })
        .await
        {
            Ok(Ok(names)) => names,
            Ok(Err(error)) => return ToolResult::error(&call.id, error),
            Err(error) => {
                return ToolResult::error(
                    &call.id,
                    format!("could not list {path}: filesystem task failed: {error}"),
                );
            }
        };
        match serde_json::to_string(&names) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(
                &call.id,
                format!("could not encode directory entries: {error}"),
            ),
        }
    }

    async fn read_file(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        let path = path.to_owned();
        match spawn_blocking({
            let path = path.clone();
            move || read_file_contents(&path)
        })
        .await
        {
            Ok(Ok(content)) => ToolResult::success(&call.id, content),
            Ok(Err(error)) => ToolResult::error(&call.id, error),
            Err(error) => ToolResult::error(
                &call.id,
                format!("could not read {path}: filesystem task failed: {error}"),
            ),
        }
    }

    async fn exec_command(
        &self,
        call: &ToolCall,
        cancellation: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> ToolResult {
        command::run(call, &self.activities, self.command_timeout, cancellation).await
    }

    fn task_list(&self, call: &ToolCall) -> ToolResult {
        match serde_json::to_string(&self.activities.activities()) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(&call.id, format!("could not encode tasks: {error}")),
        }
    }

    fn task_stop(&self, call: &ToolCall) -> ToolResult {
        let Some(id) = task_id(call) else {
            return ToolResult::error(&call.id, "arguments.task_id must have the form `task-N`");
        };
        if self.activities.stop(id) {
            ToolResult::success(&call.id, format!("stop requested for {id}"))
        } else {
            ToolResult::error(
                &call.id,
                format!("task `{id}` is unknown or already finished"),
            )
        }
    }

    async fn write_file(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        let Some(content) = call.arguments.get("content").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.content must be a string");
        };
        let path = path.to_owned();
        let content = content.to_owned();
        match spawn_blocking({
            let path = path.clone();
            move || fs::write(path, content)
        })
        .await
        {
            Ok(Ok(())) => ToolResult::success(&call.id, format!("wrote {path}")),
            Ok(Err(error)) => {
                ToolResult::error(&call.id, format!("could not write {path}: {error}"))
            }
            Err(error) => ToolResult::error(
                &call.id,
                format!("could not write {path}: filesystem task failed: {error}"),
            ),
        }
    }
}

fn builtin_definitions() -> [ToolDefinition; 8] {
    [
        ToolDefinition::new(
            "list_directory",
            "List entries in a directory.",
            serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {"path": {"type": "string"}},
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "read_file",
            "Read a UTF-8 file.",
            serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {"path": {"type": "string"}},
                "additionalProperties": false,
            }),
        ),
        image_view::definition(),
        exec_command_definition(),
        ToolDefinition::new(
            "task_list",
            "List active and recent background tasks.",
            serde_json::json!({"type": "object", "additionalProperties": false}),
        ),
        ToolDefinition::new(
            "task_stop",
            "Stop an active background task and its child processes.",
            serde_json::json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string", "pattern": "^task-[1-9][0-9]*$"}
                },
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "write_stdin",
            "Poll new output from any live exec_command session, or write to a PTY session. \
             Empty chars only polls and never closes stdin. No completion notification is sent; \
             keep polling until exit_code is returned. Non-empty input to a pipe session returns \
             StdinClosed.",
            serde_json::json!({
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": {"type": "string", "pattern": "^task-[1-9][0-9]*$"},
                    "chars": {"type": "string", "default": ""},
                    "yield_time_ms": {
                        "type": "integer", "minimum": 250, "maximum": 300000
                    },
                    "max_output_tokens": {
                        "type": "integer", "minimum": 1, "maximum": 1000000, "default": 10000
                    }
                },
                "additionalProperties": false,
            }),
        ),
        ToolDefinition::new(
            "write_file",
            "Write UTF-8 content to a file.",
            serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                },
                "additionalProperties": false,
            }),
        ),
    ]
}

fn exec_command_definition() -> ToolDefinition {
    ToolDefinition::new(
        "exec_command",
        "Run a shell command. Fast commands return inline; long commands continue as sessions. \
         No completion notification is sent to the model, so poll a returned task_id with an \
         empty write_stdin until exit_code is present. PTY mode is supported on macOS and Linux.",
        serde_json::json!({
            "type": "object",
            "required": ["cmd"],
            "properties": {
                "cmd": {"type": "string"},
                "cwd": {"type": "string"},
                "description": {"type": "string"},
                "run_in_background": {"type": "boolean", "default": false},
                "yield_time_ms": command_yield_schema(),
                "timeout_seconds": command_timeout_schema(),
                "tty": {"type": "boolean", "default": false},
                "max_output_tokens": {
                    "type": "integer", "minimum": 1, "maximum": 1000000, "default": 10000
                }
            },
            "additionalProperties": false,
        }),
    )
}

fn command_yield_schema() -> Value {
    serde_json::json!({
        "type": "integer", "minimum": 250, "maximum": 30000, "default": 10000
    })
}

fn command_timeout_schema() -> Value {
    serde_json::json!({
        "type": "integer", "minimum": 0, "maximum": 86400, "default": 0
    })
}

fn task_id(call: &ToolCall) -> Option<ActivityId> {
    call.arguments
        .get("task_id")
        .and_then(Value::as_str)
        .and_then(activity::parse_activity_id)
}

fn compile_schema(schema: &Value) -> Result<Validator, ToolRegistryError> {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(schema)
        .map_err(|error| ToolRegistryError::InvalidSchema(error.to_string()))
}

/// Reads a regular file into a string, truncating at [`MAX_READ_FILE_BYTES`] with an explicit
/// marker when the file is larger.
///
/// # Errors
///
/// Returns an error when the path is missing, is not a regular file, cannot be read, or does not
/// hold valid UTF-8.
fn read_file_contents(path: &str) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("could not read {path}: {error}"))?;
    // Opening a FIFO or a device would block a blocking-pool thread until a peer shows up, and
    // spawn_blocking tasks cannot be cancelled, so only regular files are safe to open.
    if !metadata.is_file() {
        return Err(format!(
            "read_file supports regular files only: {path} is not a regular file"
        ));
    }
    let file = fs::File::open(path).map_err(|error| format!("could not read {path}: {error}"))?;
    // Reading one byte past the cap both detects truncation and keeps the read bounded when the
    // file grows after the metadata check above.
    let mut bytes = Vec::new();
    file.take(MAX_READ_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {path}: {error}"))?;
    let truncated = bytes.len() > MAX_READ_FILE_BYTES;
    bytes.truncate(MAX_READ_FILE_BYTES);
    let mut content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            // take() can split a multi-byte character exactly at the cap; the prefix before the
            // incomplete tail is complete UTF-8 and the tail reappears in any continuation read.
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return Err(format!("could not read {path}: file is not valid UTF-8")),
    };
    if truncated {
        content.push_str(&format!(
            "\n[... truncated: showing the first {MAX_READ_FILE_BYTES} of {} bytes. \
             Use exec_command, e.g. `sed -n` or `tail`, to read the rest ...]",
            metadata.len()
        ));
    }
    Ok(content)
}

fn list_directory_entries(path: &str) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(path).map_err(|error| format!("could not list {path}: {error}"))?;
    let mut names = Vec::new();
    let mut overflow = 0_usize;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not inspect an entry in {path}: {error}"))?;
        // Entries past the cap are counted rather than collected, so a huge directory cannot
        // grow the result without bound.
        if names.len() < MAX_DIRECTORY_ENTRIES {
            names.push(entry.file_name().to_string_lossy().into_owned());
        } else {
            overflow += 1;
        }
    }
    names.sort_unstable();
    if overflow > 0 {
        names.push(format!("... and {overflow} more"));
    }
    Ok(names)
}
