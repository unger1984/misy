//! Local agent tools: their provider-visible definitions, JSON Schema validation of
//! arguments, and asynchronous execution of the built-ins. The dispatcher's definition list is
//! the single source of truth for the tools declared to the model, and per-tool budgets (read
//! and listing caps, command timeout) keep one call from growing a response without bound.
use crate::{ToolCall, ToolDefinition, ToolResult};
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, fs, io::Read};
use tokio::task::spawn_blocking;

mod command;

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
    command_timeout: std::time::Duration,
}

impl ToolDispatcher {
    /// Creates an asynchronous dispatcher backed by `registry`.
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry,
            command_timeout: command::COMMAND_TIMEOUT,
        }
    }

    /// Returns cloned definitions for every registered tool in stable name order.
    ///
    /// This is the single source of truth for the declarations sent to providers: a tool that the
    /// dispatcher can execute must also be declared to the model, so the agent builds
    /// `chat.start` params from this list rather than from a separate registry.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }

    /// Replaces the command execution timeout used by `run_command`.
    #[cfg(feature = "test-support")]
    pub fn with_command_limits(self, command_timeout: std::time::Duration) -> Self {
        Self {
            command_timeout,
            ..self
        }
    }

    /// Validates and executes one local tool call.
    ///
    /// Failures are encoded in [`ToolResult`] so a provider receives the execution outcome rather
    /// than an out-of-band core error.
    pub async fn dispatch(&self, call: &ToolCall) -> ToolResult {
        if let Err(error) = self
            .registry
            .validate_arguments(&call.name, &call.arguments)
        {
            return ToolResult::error(&call.id, error.to_string());
        }
        match call.name.as_str() {
            "list_directory" => self.list_directory(call).await,
            "read_file" => self.read_file(call).await,
            "run_command" => self.run_command(call).await,
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

    async fn run_command(&self, call: &ToolCall) -> ToolResult {
        command::run(call, self.command_timeout).await
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

fn builtin_definitions() -> [ToolDefinition; 4] {
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
        ToolDefinition::new(
            "run_command",
            "Run a local command without a shell.",
            serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {"type": "string"},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "cwd": {"type": "string"},
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
             Use run_command, e.g. `sed -n` or `tail`, to read the rest ...]",
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
