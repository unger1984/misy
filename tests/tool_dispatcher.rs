use misy::{ToolCall, ToolDispatcher, ToolRegistry};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_root(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("misy-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn dispatcher_writes_a_file_from_validated_arguments() {
    let root = test_root("write-file");
    fs::create_dir_all(&root).unwrap();
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let target = root.join("note.txt");

    let result = dispatcher.dispatch(&ToolCall::new(
        "write-1",
        "write_file",
        json!({"path": target, "content": "hello"}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(fs::read_to_string(&target).unwrap(), "hello");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dispatcher_reads_utf8_file_content() {
    let root = test_root("read-file");
    fs::create_dir_all(&root).unwrap();
    let target = root.join("note.txt");
    fs::write(&target, "hello").unwrap();
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "read-1",
        "read_file",
        json!({"path": target}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dispatcher_lists_directory_entries_in_stable_order() {
    let root = test_root("list-directory");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("zeta.txt"), "").unwrap();
    fs::write(root.join("alpha.txt"), "").unwrap();
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "list-1",
        "list_directory",
        json!({"path": root}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "[\"alpha.txt\",\"zeta.txt\"]");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn dispatcher_runs_a_command_without_a_shell() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "command-1",
        "run_command",
        json!({"command": "printf", "args": ["hello"]}),
    ));

    assert!(!result.is_error, "{}", result.content);
    let output: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["stdout"], "hello");
}
