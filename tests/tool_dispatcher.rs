//! Tool-dispatch integration tests.

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
        .expect("system time must be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("misy-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn dispatcher_writes_a_file_from_validated_arguments() {
    let root = test_root("write-file");
    fs::create_dir_all(&root).expect("create write-file test directory");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let target = root.join("note.txt");

    let result = dispatcher.dispatch(&ToolCall::new(
        "write-1",
        "write_file",
        json!({"path": target, "content": "hello"}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        fs::read_to_string(&target).expect("read written file"),
        "hello"
    );
    fs::remove_dir_all(root).expect("remove write-file test directory");
}

#[test]
fn dispatcher_reads_utf8_file_content() {
    let root = test_root("read-file");
    fs::create_dir_all(&root).expect("create read-file test directory");
    let target = root.join("note.txt");
    fs::write(&target, "hello").expect("write fixture file");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "read-1",
        "read_file",
        json!({"path": target}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello");
    fs::remove_dir_all(root).expect("remove read-file test directory");
}

#[test]
fn dispatcher_lists_directory_entries_in_stable_order() {
    let root = test_root("list-directory");
    fs::create_dir_all(&root).expect("create list-directory test directory");
    fs::write(root.join("zeta.txt"), "").expect("write zeta fixture");
    fs::write(root.join("alpha.txt"), "").expect("write alpha fixture");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "list-1",
        "list_directory",
        json!({"path": root}),
    ));

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "[\"alpha.txt\",\"zeta.txt\"]");
    fs::remove_dir_all(root).expect("remove list-directory test directory");
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
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse command output");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["stdout"], "hello");
}

#[cfg(unix)]
#[test]
fn dispatcher_reports_command_timeout_after_killing_and_reaping_the_child() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new())
        .with_command_limits(std::time::Duration::from_millis(50));

    let result = dispatcher.dispatch(&ToolCall::new(
        "command-timeout",
        "run_command",
        json!({"command": "sleep", "args": ["10"]}),
    ));

    assert!(result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse timeout output");
    assert_eq!(output["kind"], "timeout");
    assert_eq!(output["exit_code"], serde_json::Value::Null);
}

#[cfg(unix)]
#[test]
fn dispatcher_marks_command_output_that_exceeds_its_bound() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "command-truncated",
        "run_command",
        json!({"command": "sh", "args": ["-c", "yes x | head -c 70000"]}),
    ));

    assert!(result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse truncated output");
    assert_eq!(output["kind"], "truncated");
    assert_eq!(output["stdout_truncated"], true);
    assert!(
        output["stdout"]
            .as_str()
            .expect("stdout must be a string")
            .len()
            < 70000
    );
}

#[cfg(unix)]
#[test]
fn dispatcher_marks_stderr_that_exceeds_its_bound() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher.dispatch(&ToolCall::new(
        "command-stderr-truncated",
        "run_command",
        json!({"command": "sh", "args": ["-c", "yes x | head -c 70000 >&2"]}),
    ));

    assert!(result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse truncated output");
    assert_eq!(output["kind"], "truncated");
    assert_eq!(output["stderr_truncated"], true);
    assert!(
        output["stderr"]
            .as_str()
            .expect("stderr must be a string")
            .len()
            < 70000
    );
}

#[cfg(unix)]
#[test]
fn dispatcher_labels_nonzero_exit_and_spawn_failures() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let nonzero = dispatcher.dispatch(&ToolCall::new(
        "command-nonzero",
        "run_command",
        json!({"command": "sh", "args": ["-c", "exit 7"]}),
    ));
    assert!(nonzero.is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&nonzero.content)
            .expect("parse nonzero command output")["kind"],
        "nonzero_exit"
    );

    let spawn = dispatcher.dispatch(&ToolCall::new(
        "command-spawn",
        "run_command",
        json!({"command": "misy-command-that-does-not-exist"}),
    ));
    assert!(spawn.is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&spawn.content)
            .expect("parse spawn failure output")["kind"],
        "spawn_error"
    );
}
