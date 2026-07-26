use crate::{ToolCall, ToolDefinition, ToolResult};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error, fmt, fs, process::Command};

/// Definitions available to providers. Built-ins are installed on construction.
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let definitions = builtin_definitions()
            .into_iter()
            .map(|definition| (definition.name.clone(), definition))
            .collect();
        Self { definitions }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.definitions.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name)
    }

    pub fn register(&mut self, definition: ToolDefinition) -> Result<(), ToolRegistryError> {
        if self.definitions.contains_key(&definition.name) {
            return Err(ToolRegistryError::DuplicateTool(definition.name));
        }
        self.definitions.insert(definition.name.clone(), definition);
        Ok(())
    }

    pub fn validate_arguments(
        &self,
        name: &str,
        arguments: &Value,
    ) -> Result<(), ToolRegistryError> {
        let definition = self
            .get(name)
            .ok_or_else(|| ToolRegistryError::UnknownTool(name.to_owned()))?;
        validate_schema(&definition.input_schema, arguments, "arguments")
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
    UnknownTool(String),
    InvalidArguments(String),
}

impl fmt::Display for ToolRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool(name) => write!(formatter, "tool `{name}` is already registered"),
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
        let output = match command.output() {
            Ok(output) => output,
            Err(error) => {
                return ToolResult::error(
                    &call.id,
                    format!("could not run {command_name}: {error}"),
                );
            }
        };
        let content = serde_json::json!({
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        })
        .to_string();
        if output.status.success() {
            ToolResult::success(&call.id, content)
        } else {
            ToolResult::error(&call.id, content)
        }
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

fn validate_schema(schema: &Value, value: &Value, path: &str) -> Result<(), ToolRegistryError> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            unsupported => {
                return Err(ToolRegistryError::InvalidArguments(format!(
                    "{path} uses unsupported JSON Schema type `{unsupported}`"
                )));
            }
        };
        if !matches {
            return Err(ToolRegistryError::InvalidArguments(format!(
                "{path} must be a {expected}"
            )));
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return Err(ToolRegistryError::InvalidArguments(format!(
            "{path} must be one of the declared enum values"
        )));
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(ToolRegistryError::InvalidArguments(format!(
                        "{path}.{field} is required"
                    )));
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && let Some(properties) = properties
        {
            for field in object.keys() {
                if !properties.contains_key(field) {
                    return Err(ToolRegistryError::InvalidArguments(format!(
                        "{path}.{field} is not allowed"
                    )));
                }
            }
        }
        if let Some(properties) = properties {
            for (field, field_schema) in properties {
                if let Some(field_value) = object.get(field) {
                    validate_schema(field_schema, field_value, &format!("{path}.{field}"))?;
                }
            }
        }
    }
    if let Some(values) = value.as_array()
        && let Some(item_schema) = schema.get("items")
    {
        for (index, item) in values.iter().enumerate() {
            validate_schema(item_schema, item, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}
