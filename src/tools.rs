use crate::{ToolCall, ToolDefinition, ToolResult};
use command_group::{CommandGroup, GroupChild};
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    io::{self, Read},
    process::{Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

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

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name).map(|tool| &tool.definition)
    }

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

#[derive(Debug, Eq, PartialEq)]
pub enum ToolRegistryError {
    DuplicateTool(String),
    InvalidSchema(String),
    UnknownTool(String),
    InvalidArguments(String),
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool(name) => write!(formatter, "tool `{name}` is already registered"),
            Self::InvalidSchema(message) => write!(formatter, "invalid JSON Schema: {message}"),
            Self::UnknownTool(name) => write!(formatter, "unknown tool `{name}`"),
            Self::InvalidArguments(message) => formatter.write_str(message),
        }
    }
}

impl Error for ToolRegistryError {}

/// Executes registered built-in tools synchronously; callers retain source-order dispatch.
#[derive(Clone, Debug)]
pub struct ToolDispatcher {
    registry: ToolRegistry,
}

impl ToolDispatcher {
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }

    pub fn dispatch(&self, call: &ToolCall) -> ToolResult {
        if let Err(error) = self
            .registry
            .validate_arguments(&call.name, &call.arguments)
        {
            return ToolResult::error(&call.id, error.to_string());
        }
        match call.name.as_str() {
            "list_directory" => self.list_directory(call),
            "read_file" => self.read_file(call),
            "run_command" => self.run_command(call),
            "write_file" => self.write_file(call),
            _ => ToolResult::error(&call.id, format!("tool `{}` is not executable", call.name)),
        }
    }

    fn list_directory(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                return ToolResult::error(&call.id, format!("could not list {path}: {error}"));
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
                Err(error) => {
                    return ToolResult::error(
                        &call.id,
                        format!("could not inspect an entry in {path}: {error}"),
                    );
                }
            }
        }
        names.sort_unstable();
        match serde_json::to_string(&names) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(
                &call.id,
                format!("could not encode directory entries: {error}"),
            ),
        }
    }

    fn read_file(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        match fs::read_to_string(path) {
            Ok(content) => ToolResult::success(&call.id, content),
            Err(error) => ToolResult::error(&call.id, format!("could not read {path}: {error}")),
        }
    }

    fn run_command(&self, call: &ToolCall) -> ToolResult {
        let Some(command_name) = call.arguments.get("command").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.command must be a string");
        };
        let mut command = Command::new(command_name);
        if let Some(arguments) = call.arguments.get("args").and_then(Value::as_array) {
            for argument in arguments {
                let Some(argument) = argument.as_str() else {
                    return ToolResult::error(&call.id, "arguments.args must contain strings");
                };
                command.arg(argument);
            }
        }
        if let Some(working_directory) = call.arguments.get("cwd").and_then(Value::as_str) {
            command.current_dir(working_directory);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.group_spawn() {
            Ok(child) => child,
            Err(error) => {
                return command_result(
                    &call.id,
                    "spawn_error",
                    None,
                    CapturedStream::default(),
                    CapturedStream::default(),
                    Some(format!("could not run {command_name}: {error}")),
                );
            }
        };
        let stdout = capture_stream(
            child
                .inner()
                .stdout
                .take()
                .expect("piped stdout is available after spawn"),
        );
        let stderr = capture_stream(
            child
                .inner()
                .stderr
                .take()
                .expect("piped stderr is available after spawn"),
        );
        let status = match wait_for_command(&mut child) {
            Ok(Some(status)) => status,
            Ok(None) => {
                let termination_error = terminate_and_reap(&mut child)
                    .err()
                    .map(|error| error.to_string());
                return command_result(
                    &call.id,
                    "timeout",
                    None,
                    join_capture(stdout),
                    join_capture(stderr),
                    termination_error,
                );
            }
            Err(error) => {
                let termination_error = terminate_and_reap(&mut child)
                    .err()
                    .map(|error| error.to_string());
                return command_result(
                    &call.id,
                    "wait_error",
                    None,
                    join_capture(stdout),
                    join_capture(stderr),
                    Some(format!("could not wait for {command_name}: {error}"))
                        .or(termination_error),
                );
            }
        };
        let stdout = join_capture(stdout);
        let stderr = join_capture(stderr);
        let kind = if stdout.truncated || stderr.truncated {
            "truncated"
        } else if status.success() {
            "success"
        } else {
            "nonzero_exit"
        };
        command_result(&call.id, kind, Some(status), stdout, stderr, None)
    }

    fn write_file(&self, call: &ToolCall) -> ToolResult {
        let Some(path) = call.arguments.get("path").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.path must be a string");
        };
        let Some(content) = call.arguments.get("content").and_then(Value::as_str) else {
            return ToolResult::error(&call.id, "arguments.content must be a string");
        };
        match fs::write(path, content) {
            Ok(()) => ToolResult::success(&call.id, format!("wrote {path}")),
            Err(error) => ToolResult::error(&call.id, format!("could not write {path}: {error}")),
        }
    }
}

fn builtin_definitions() -> [ToolDefinition; 4] {
    [
        ToolDefinition::new(
            "list_directory",
            "List entries in a directory.",
            serde_json::json!({"type":"object", "required":["path"], "properties":{"path":{"type":"string"}}, "additionalProperties":false}),
        ),
        ToolDefinition::new(
            "read_file",
            "Read a UTF-8 file.",
            serde_json::json!({"type":"object", "required":["path"], "properties":{"path":{"type":"string"}}, "additionalProperties":false}),
        ),
        ToolDefinition::new(
            "run_command",
            "Run a local command without a shell.",
            serde_json::json!({"type":"object", "required":["command"], "properties":{"command":{"type":"string"}, "args":{"type":"array", "items":{"type":"string"}}, "cwd":{"type":"string"}}, "additionalProperties":false}),
        ),
        ToolDefinition::new(
            "write_file",
            "Write UTF-8 content to a file.",
            serde_json::json!({"type":"object", "required":["path", "content"], "properties":{"path":{"type":"string"}, "content":{"type":"string"}}, "additionalProperties":false}),
        ),
    ]
}

fn compile_schema(schema: &Value) -> Result<Validator, ToolRegistryError> {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(schema)
        .map_err(|error| ToolRegistryError::InvalidSchema(error.to_string()))
}

#[derive(Default)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

fn capture_stream<R>(mut reader: R) -> JoinHandle<io::Result<CapturedStream>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = CapturedStream::default();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                return Ok(captured);
            }
            let available = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(captured.bytes.len());
            let kept = available.min(read);
            captured.bytes.extend_from_slice(&buffer[..kept]);
            captured.truncated |= kept < read;
        }
    })
}

fn join_capture(handle: JoinHandle<io::Result<CapturedStream>>) -> CapturedStream {
    handle.join().ok().and_then(Result::ok).unwrap_or_default()
}

fn wait_for_command(child: &mut GroupChild) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn terminate_and_reap(child: &mut GroupChild) -> io::Result<ExitStatus> {
    match child.kill() {
        Ok(()) => child.wait(),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => child.wait(),
        Err(error) => Err(error),
    }
}

fn command_result(
    call_id: &str,
    kind: &str,
    status: Option<ExitStatus>,
    stdout: CapturedStream,
    stderr: CapturedStream,
    message: Option<String>,
) -> ToolResult {
    let content = serde_json::json!({
        "kind": kind,
        "exit_code": status.and_then(|status| status.code()),
        "stdout": String::from_utf8_lossy(&stdout.bytes),
        "stderr": String::from_utf8_lossy(&stderr.bytes),
        "stdout_truncated": stdout.truncated,
        "stderr_truncated": stderr.truncated,
        "message": message,
    })
    .to_string();
    if kind == "success" {
        ToolResult::success(call_id, content)
    } else {
        ToolResult::error(call_id, content)
    }
}
