//! Tool-dispatch integration tests.

use misy_core::{ToolCall, ToolDefinition, ToolDispatcher, ToolRegistry};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn test_root(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("misy-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn dispatcher_declares_every_registered_tool() {
    // The agent builds chat.start params from dispatcher.definitions(), so a tool registered
    // through the public ToolRegistry::register must be visible there, not only executable.
    let mut registry = ToolRegistry::new();
    registry
        .register(ToolDefinition::new(
            "custom_echo",
            "Echo a value back.",
            json!({"type": "object", "properties": {"value": {"type": "string"}}}),
        ))
        .expect("register custom test tool");
    let dispatcher = ToolDispatcher::new(registry);

    let definitions = dispatcher.definitions();
    let names: Vec<_> = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "custom_echo",
            "list_directory",
            "read_file",
            "run_command",
            "view_image",
            "write_file"
        ]
    );
}

#[tokio::test]
async fn dispatcher_returns_image_content_for_view_image() {
    let root = test_root("view-image");
    fs::create_dir_all(&root).expect("create view-image test directory");
    let target = root.join("pixel.png");
    let image = misy_core::ImageAttachment::from_rgba(1, 1, vec![255, 0, 0, 255])
        .expect("encode test image");
    fs::write(&target, image.bytes()).expect("write image fixture");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "image-1",
            "view_image",
            json!({"path": target}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.attachments.len(), 1);
    assert_eq!(result.attachments[0].media_type(), "image/png");
    fs::remove_dir_all(root).expect("remove view-image test directory");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_rejects_fifo_image_reads_without_waiting_for_a_writer() {
    let root = test_root("view-image-fifo");
    fs::create_dir_all(&root).expect("create view-image-fifo test directory");
    let target = root.join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&target)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo must succeed");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.dispatch(&ToolCall::new(
            "image-fifo",
            "view_image",
            json!({"path": target}),
        )),
    )
    .await
    .expect("view_image must not wait for a FIFO writer");

    assert!(result.is_error);
    assert!(
        result.content.contains("regular file"),
        "{}",
        result.content
    );
    fs::remove_dir_all(root).expect("remove view-image-fifo test directory");
}

#[tokio::test]
async fn dispatcher_writes_a_file_from_validated_arguments() {
    let root = test_root("write-file");
    fs::create_dir_all(&root).expect("create write-file test directory");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let target = root.join("note.txt");

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "write-1",
            "write_file",
            json!({"path": target, "content": "hello"}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        fs::read_to_string(&target).expect("read written file"),
        "hello"
    );
    fs::remove_dir_all(root).expect("remove write-file test directory");
}

#[tokio::test]
async fn dispatcher_reads_utf8_file_content() {
    let root = test_root("read-file");
    fs::create_dir_all(&root).expect("create read-file test directory");
    let target = root.join("note.txt");
    fs::write(&target, "hello").expect("write fixture file");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "read-1",
            "read_file",
            json!({"path": target}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello");
    fs::remove_dir_all(root).expect("remove read-file test directory");
}

#[tokio::test]
async fn dispatcher_lists_directory_entries_in_stable_order() {
    let root = test_root("list-directory");
    fs::create_dir_all(&root).expect("create list-directory test directory");
    fs::write(root.join("zeta.txt"), "").expect("write zeta fixture");
    fs::write(root.join("alpha.txt"), "").expect("write alpha fixture");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "list-1",
            "list_directory",
            json!({"path": root}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "[\"alpha.txt\",\"zeta.txt\"]");
    fs::remove_dir_all(root).expect("remove list-directory test directory");
}

#[tokio::test]
async fn dispatcher_rejects_reading_a_directory_as_a_file() {
    let root = test_root("read-directory");
    fs::create_dir_all(&root).expect("create read-directory test directory");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "read-directory",
            "read_file",
            json!({"path": root}),
        ))
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("regular files only"),
        "{}",
        result.content
    );
    fs::remove_dir_all(root).expect("remove read-directory test directory");
}

#[tokio::test]
async fn dispatcher_truncates_files_larger_than_the_read_cap() {
    let root = test_root("read-large");
    fs::create_dir_all(&root).expect("create read-large test directory");
    let target = root.join("large.txt");
    let mut content = String::new();
    while content.len() <= 4 * 1024 * 1024 + 1024 {
        content.push_str(&"x".repeat(1024));
    }
    fs::write(&target, &content).expect("write large fixture file");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "read-large",
            "read_file",
            json!({"path": target}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.len() < content.len(),
        "the result must not hold the whole file"
    );
    assert!(
        result
            .content
            .contains(&format!("of {} bytes", content.len())),
        "{}",
        &result.content[result.content.len() - 200..]
    );
    fs::remove_dir_all(root).expect("remove read-large test directory");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_rejects_fifo_reads_without_waiting_for_a_writer() {
    let root = test_root("read-fifo");
    fs::create_dir_all(&root).expect("create read-fifo test directory");
    let target = root.join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&target)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo must succeed");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        dispatcher.dispatch(&ToolCall::new(
            "read-fifo",
            "read_file",
            json!({"path": target}),
        )),
    )
    .await
    .expect("read_file must not wait for a FIFO writer");

    assert!(result.is_error);
    assert!(
        result.content.contains("regular files only"),
        "{}",
        result.content
    );
    fs::remove_dir_all(root).expect("remove read-fifo test directory");
}

#[tokio::test]
async fn dispatcher_caps_directory_listings_with_a_marker() {
    let root = test_root("list-overflow");
    fs::create_dir_all(&root).expect("create list-overflow test directory");
    for index in 0..10_001 {
        fs::write(root.join(format!("entry-{index:05}.txt")), "").expect("write overflow fixture");
    }
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "list-overflow",
            "list_directory",
            json!({"path": root}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    let entries: Vec<String> =
        serde_json::from_str(&result.content).expect("parse directory entries");
    assert_eq!(entries.len(), 10_001);
    assert_eq!(entries.last().map(String::as_str), Some("... and 1 more"));
    fs::remove_dir_all(root).expect("remove list-overflow test directory");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_runs_a_command_without_a_shell() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "command-1",
            "run_command",
            json!({"command": "printf", "args": ["hello"]}),
        ))
        .await;

    assert!(!result.is_error, "{}", result.content);
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse command output");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(output["stdout"], "hello");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_reports_command_timeout_after_killing_and_reaping_the_child() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new())
        .with_command_limits(std::time::Duration::from_millis(50));

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "command-timeout",
            "run_command",
            json!({"command": "sleep", "args": ["10"]}),
        ))
        .await;

    assert!(result.is_error);
    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse timeout output");
    assert_eq!(output["kind"], "timeout");
    assert_eq!(output["exit_code"], serde_json::Value::Null);
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_reaps_a_command_when_its_caller_is_cancelled() {
    let root = test_root("cancelled-command");
    fs::create_dir_all(&root).expect("create cancelled-command test directory");
    let started = root.join("started");
    let escaped = root.join("escaped");
    let script = format!(
        "touch {}; sleep 1; touch {}",
        started.display(),
        escaped.display()
    );
    let dispatcher = Arc::new(
        ToolDispatcher::new(ToolRegistry::new()).with_command_limits(Duration::from_millis(50)),
    );
    let call = ToolCall::new(
        "command-cancelled",
        "run_command",
        json!({"command": "sh", "args": ["-c", script]}),
    );
    let task = tokio::spawn({
        let dispatcher = Arc::clone(&dispatcher);
        async move { dispatcher.dispatch(&call).await }
    });

    for _ in 0..20 {
        if started.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.exists(),
        "command must begin before its caller is cancelled"
    );
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        !escaped.exists(),
        "the detached caller must not leave the command running"
    );
    fs::remove_dir_all(root).expect("remove cancelled-command test directory");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_returns_when_a_detached_grandchild_holds_the_output_pipe() {
    let root = test_root("detached-grandchild");
    fs::create_dir_all(&root).expect("create detached-grandchild test directory");
    let escaped = root.join("escaped");
    // A plain `(sleep 30 &)` stays in the killed process group, so the escape
    // needs setsid; the marker file guarantees the grandchild has left the
    // group before the leader exits and the group kill is sent.
    // The leader prints before daemonizing, so the run also proves the drain deadline keeps
    // what was already read instead of discarding it with the aborted capture task.
    let script = format!(
        "echo 'printed before the daemon held the pipe'; \
         (perl -MPOSIX=setsid -e 'setsid(); \
         open(my $marker, \">\", $ARGV[0]); exec \"sleep\", \"30\"' '{}' &) ; \
         for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
         [ -e '{}' ] && break; sleep 0.1; done; exit 0",
        escaped.display(),
        escaped.display()
    );
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    // The command exits almost at once, but the escaped grandchild inherits
    // the stdout pipe and never closes it, so capture EOF does not arrive on
    // its own; the dispatch must still return instead of joining forever.
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        dispatcher.dispatch(&ToolCall::new(
            "command-detached-grandchild",
            "run_command",
            json!({"command": "sh", "args": ["-c", script]}),
        )),
    )
    .await
    .expect("run must return even when a grandchild holds the output pipe open");

    let output: serde_json::Value =
        serde_json::from_str(&result.content).expect("parse command output");
    assert_eq!(output["exit_code"], 0);
    assert_eq!(
        output["stdout"], "printed before the daemon held the pipe\n",
        "output read before the drain deadline must survive the aborted capture"
    );
    // The pipe never reached EOF, so the captured output is real but incomplete. It travels the
    // same path as a budget overflow: flagged truncated rather than presented as the full output.
    assert_eq!(output["kind"], "truncated");
    assert_eq!(output["stdout_truncated"], true);
    assert!(result.is_error);
    fs::remove_dir_all(root).expect("remove detached-grandchild test directory");
}

#[cfg(unix)]
#[tokio::test]
async fn dispatcher_marks_command_output_that_exceeds_its_bound() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "command-truncated",
            "run_command",
            json!({"command": "sh", "args": ["-c", "yes x | head -c 70000"]}),
        ))
        .await;

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
#[tokio::test]
async fn dispatcher_marks_stderr_that_exceeds_its_bound() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let result = dispatcher
        .dispatch(&ToolCall::new(
            "command-stderr-truncated",
            "run_command",
            json!({"command": "sh", "args": ["-c", "yes x | head -c 70000 >&2"]}),
        ))
        .await;

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
#[tokio::test]
async fn dispatcher_labels_nonzero_exit_and_spawn_failures() {
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let nonzero = dispatcher
        .dispatch(&ToolCall::new(
            "command-nonzero",
            "run_command",
            json!({"command": "sh", "args": ["-c", "exit 7"]}),
        ))
        .await;
    assert!(nonzero.is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&nonzero.content)
            .expect("parse nonzero command output")["kind"],
        "nonzero_exit"
    );

    let spawn = dispatcher
        .dispatch(&ToolCall::new(
            "command-spawn",
            "run_command",
            json!({"command": "misy-command-that-does-not-exist"}),
        ))
        .await;
    assert!(spawn.is_error);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&spawn.content)
            .expect("parse spawn failure output")["kind"],
        "spawn_error"
    );
}
