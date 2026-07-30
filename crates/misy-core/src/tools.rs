//! Local agent tools: their provider-visible definitions, JSON Schema validation of
//! arguments, and asynchronous execution of the built-ins. The dispatcher's definition list is
//! the single source of truth for the tools declared to the model, and per-tool budgets (read
//! and listing caps, command timeout) keep one call from growing a response without bound.
use crate::{
    ActivityId, ActivityOutput, ActivitySummary, ToolCall, ToolDefinition, ToolResult,
    activity::ActivityOwner,
};
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, fs};
use tokio::task::spawn_blocking;

mod activity;
mod command;
mod definitions;
mod filesystem;
mod image_view;
mod stdin;
#[cfg(test)]
mod tests;

pub(crate) use activity::ActivityEvent;

/// Definitions available to providers. Built-ins are installed on construction.
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, RegisteredTool>,
}

#[derive(Clone, Debug)]
struct RegisteredTool {
    definition: ToolDefinition,
    validator: Validator,
    scope_policy: ToolScopePolicy,
}

/// Filesystem scope required before one validated tool call may execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolScopePolicy {
    /// A string argument names the filesystem target.
    PathArgument(&'static str),
    /// An optional string argument selects the command working directory.
    WorkingDirectory(&'static str),
    /// A task identifier inherits the immutable cwd of its command activity.
    ActivityWorkingDirectory(&'static str),
    /// The tool has no filesystem side effect requiring hierarchical instructions.
    None,
}

impl ToolRegistry {
    /// Creates a registry containing all built-in tool definitions.
    ///
    /// # Panics
    ///
    /// Panics only if a statically defined built-in schema is invalid, which is a build-time
    /// invariant covered by tests.
    pub fn new() -> Self {
        let definitions = definitions::builtin_definitions()
            .into_iter()
            .chain(crate::core::agents::tool_definitions())
            .map(|definition| {
                let validator = compile_schema(&definition.input_schema)
                    .expect("built-in tool schemas must be valid JSON Schema draft 2020-12");
                let scope_policy = builtin_scope_policy(&definition.name)
                    .expect("every built-in tool must declare an instruction scope policy");
                (
                    definition.name.clone(),
                    RegisteredTool {
                        definition,
                        validator,
                        scope_policy,
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
                scope_policy: ToolScopePolicy::None,
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
    activity_ids: std::sync::Arc<std::sync::atomic::AtomicU64>,
    command_timeout: Option<std::time::Duration>,
}

impl ToolDispatcher {
    /// Creates an asynchronous dispatcher backed by `registry`.
    pub fn new(registry: ToolRegistry) -> Self {
        let activity_ids = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        Self {
            registry,
            activities: activity::ActivityManager::with_ids(std::sync::Arc::clone(&activity_ids)),
            activity_ids,
            command_timeout: None,
        }
    }

    /// Allocates one identifier shared by command and child-agent activities.
    pub(crate) fn next_activity_id(&self) -> ActivityId {
        ActivityId::new(
            self.activity_ids
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
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

    pub(crate) fn definition_names(&self) -> std::collections::BTreeSet<String> {
        self.registry.definitions.keys().cloned().collect()
    }

    pub(crate) fn definitions_for_agent(
        &self,
        supports_images: bool,
        supports_questions: bool,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<ToolDefinition> {
        self.definitions_for_client(supports_images, supports_questions)
            .into_iter()
            .filter(|definition| allowed.is_none_or(|tools| tools.contains(&definition.name)))
            .collect()
    }

    pub(crate) fn definitions_for_client(
        &self,
        supports_images: bool,
        supports_questions: bool,
    ) -> Vec<ToolDefinition> {
        self.definitions_for(supports_images)
            .into_iter()
            .filter(|definition| supports_questions || definition.name != "AskUserQuestion")
            .collect()
    }

    pub(crate) fn validate_arguments(&self, call: &ToolCall) -> Result<(), ToolRegistryError> {
        self.registry
            .validate_arguments(&call.name, &call.arguments)
    }

    pub(crate) fn scope_policy(&self, name: &str) -> Result<ToolScopePolicy, ToolRegistryError> {
        self.registry
            .definitions
            .get(name)
            .map(|tool| tool.scope_policy)
            .ok_or_else(|| ToolRegistryError::UnknownTool(name.to_owned()))
    }

    pub(crate) fn activity_cwd(&self, call: &ToolCall, owner: ActivityOwner) -> Option<String> {
        let id = task_id(call)?;
        self.activities.cwd_for_owner(owner, id)
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
        self.dispatch_inner(call, ActivityOwner::Main, None).await
    }

    pub(crate) async fn dispatch_cancellable(
        &self,
        call: &ToolCall,
        cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> ToolResult {
        self.dispatch_inner(call, ActivityOwner::Main, Some(cancellation))
            .await
    }

    pub(crate) async fn dispatch_for_owner(
        &self,
        call: &ToolCall,
        owner: ActivityOwner,
        cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> ToolResult {
        self.dispatch_inner(call, owner, Some(cancellation)).await
    }

    async fn dispatch_inner(
        &self,
        call: &ToolCall,
        owner: ActivityOwner,
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
            "exec_command" => self.exec_command(call, owner, cancellation).await,
            "task_list" => self.task_list(call, owner),
            "task_stop" => self.task_stop(call, owner),
            "write_stdin" => stdin::execute(call, owner, &self.activities, cancellation).await,
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
            move || filesystem::list_directory_entries(&path)
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
            move || filesystem::read_file_contents(&path)
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
        owner: ActivityOwner,
        cancellation: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> ToolResult {
        command::run(
            call,
            owner,
            &self.activities,
            self.command_timeout,
            cancellation,
        )
        .await
    }

    fn task_list(&self, call: &ToolCall, owner: ActivityOwner) -> ToolResult {
        match serde_json::to_string(&self.activities.activities_for_owner(owner)) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(&call.id, format!("could not encode tasks: {error}")),
        }
    }

    fn task_stop(&self, call: &ToolCall, owner: ActivityOwner) -> ToolResult {
        let Some(id) = task_id(call) else {
            return ToolResult::error(&call.id, "arguments.task_id must have the form `task-N`");
        };
        if self.activities.stop_for_owner(owner, id) {
            ToolResult::success(&call.id, format!("stop requested for {id}"))
        } else {
            ToolResult::error(
                &call.id,
                format!("task `{id}` is unknown or already finished"),
            )
        }
    }

    pub(crate) async fn stop_owner_commands(&self, owner: ActivityOwner) {
        self.activities.stop_owner(owner).await;
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

fn builtin_scope_policy(name: &str) -> Option<ToolScopePolicy> {
    match name {
        "list_directory" | "read_file" | "view_image" | "write_file" => {
            Some(ToolScopePolicy::PathArgument("path"))
        }
        "exec_command" => Some(ToolScopePolicy::WorkingDirectory("cwd")),
        "write_stdin" => Some(ToolScopePolicy::ActivityWorkingDirectory("task_id")),
        "task_list" | "task_stop" | "spawn_agent" | "agent_list" | "agent_wait"
        | "agent_output" | "agent_message" | "agent_stop" | "model_search" | "SetTodoList"
        | "AskUserQuestion" => Some(ToolScopePolicy::None),
        _ => None,
    }
}
