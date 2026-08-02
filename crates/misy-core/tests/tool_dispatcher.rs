//! Tool-dispatch integration tests.

use misy_core::{ToolCall, ToolDefinition, ToolDispatcher, ToolRegistry};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
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
            "AskUserQuestion",
            "SetTodoList",
            "StrReplaceFile",
            "agent_list",
            "agent_message",
            "agent_output",
            "agent_stop",
            "agent_wait",
            "custom_echo",
            "exec_command",
            "list_directory",
            "model_search",
            "read_file",
            "spawn_agent",
            "task_list",
            "task_stop",
            "view_image",
            "write_file",
            "write_stdin"
        ]
    );
}

#[tokio::test]
async fn dispatcher_replaces_unique_and_all_non_overlapping_fragments() {
    let root = test_root("string-replace-success");
    fs::create_dir_all(&root).expect("create string replacement test directory");
    let target = root.join("note.txt");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    fs::write(&target, "before middle before").expect("write repeated fixture");
    let unique = dispatcher
        .dispatch(&ToolCall::new(
            "replace-unique",
            "StrReplaceFile",
            json!({"path": target, "old": "middle", "new": "center"}),
        ))
        .await;
    assert!(!unique.is_error, "{}", unique.content);
    assert!(unique.content.contains("1 replacement"));
    assert_eq!(
        fs::read_to_string(&target).expect("read unique replacement"),
        "before center before"
    );

    let all = dispatcher
        .dispatch(&ToolCall::new(
            "replace-all",
            "StrReplaceFile",
            json!({"path": target, "old": "before", "new": "after", "replace_all": true}),
        ))
        .await;
    assert!(!all.is_error, "{}", all.content);
    assert!(all.content.contains("2 replacements"));
    assert_eq!(
        fs::read_to_string(&target).expect("read all replacements"),
        "after center after"
    );

    fs::write(&target, "aaaa").expect("write overlap fixture");
    let overlapping = dispatcher
        .dispatch(&ToolCall::new(
            "replace-overlap",
            "StrReplaceFile",
            json!({"path": target, "old": "aa", "new": "b", "replace_all": true}),
        ))
        .await;
    assert!(!overlapping.is_error, "{}", overlapping.content);
    assert!(overlapping.content.contains("2 replacements"));
    assert_eq!(
        fs::read_to_string(&target).expect("read overlap replacement"),
        "bb"
    );
    fs::remove_dir_all(root).expect("remove string replacement test directory");
}

#[tokio::test]
async fn dispatcher_rejects_invalid_string_replacements_without_mutation() {
    let root = test_root("string-replace-invalid");
    fs::create_dir_all(&root).expect("create string replacement test directory");
    let target = root.join("note.txt");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let cases = [
        (
            "absent",
            json!({"path": target, "old": "missing", "new": "after"}),
        ),
        (
            "ambiguous",
            json!({"path": target, "old": "before", "new": "after"}),
        ),
        ("empty", json!({"path": target, "old": "", "new": "after"})),
        (
            "no-op",
            json!({"path": target, "old": "before", "new": "before"}),
        ),
        (
            "zero-all",
            json!({"path": target, "old": "missing", "new": "after", "replace_all": true}),
        ),
    ];
    for (id, arguments) in cases {
        fs::write(&target, "before before").expect("reset fixture");
        let result = dispatcher
            .dispatch(&ToolCall::new(id, "StrReplaceFile", arguments))
            .await;
        assert!(result.is_error, "{} must fail", result.content);
        assert_eq!(
            fs::read_to_string(&target).expect("read unchanged fixture"),
            "before before"
        );
    }
    let missing = root.join("missing.txt");
    let result = dispatcher
        .dispatch(&ToolCall::new(
            "missing",
            "StrReplaceFile",
            json!({"path": missing, "old": "before", "new": "after"}),
        ))
        .await;
    assert!(result.is_error);
    assert!(!missing.exists());
    let directory = root.join("directory-target");
    fs::create_dir(&directory).expect("create non-regular target");
    let non_regular = dispatcher
        .dispatch(&ToolCall::new(
            "non-regular",
            "StrReplaceFile",
            json!({"path": directory, "old": "before", "new": "after"}),
        ))
        .await;
    assert!(non_regular.is_error);
    assert!(directory.is_dir());
    fs::write(&target, [0xff]).expect("write invalid UTF-8 fixture");
    let invalid_utf8 = dispatcher
        .dispatch(&ToolCall::new(
            "invalid-utf8",
            "StrReplaceFile",
            json!({"path": target, "old": "before", "new": "after"}),
        ))
        .await;
    assert!(invalid_utf8.is_error);
    assert_eq!(
        fs::read(&target).expect("read invalid UTF-8 fixture"),
        vec![0xff]
    );
    fs::remove_dir_all(root).expect("remove string replacement test directory");
}

#[tokio::test]
async fn dispatcher_enforces_string_replacement_size_boundaries_without_mutation() {
    const MAX_TEXT_FILE_BYTES: usize = 4 * 1024 * 1024;
    let root = test_root("string-replace-boundaries");
    fs::create_dir_all(&root).expect("create string replacement test directory");
    let target = root.join("note.txt");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    fs::write(&target, format!("z{}", "x".repeat(MAX_TEXT_FILE_BYTES - 2)))
        .expect("write exact output fixture");
    let exact = dispatcher
        .dispatch(&ToolCall::new(
            "exact",
            "StrReplaceFile",
            json!({"path": target, "old": "z", "new": "zz"}),
        ))
        .await;
    assert!(!exact.is_error, "{}", exact.content);
    assert_eq!(
        fs::metadata(&target).expect("stat exact output").len(),
        MAX_TEXT_FILE_BYTES as u64
    );

    fs::write(&target, "x".repeat(MAX_TEXT_FILE_BYTES)).expect("write amplification fixture");
    let original = fs::read(&target).expect("read amplification fixture");
    let amplified = dispatcher
        .dispatch(&ToolCall::new(
            "amplified",
            "StrReplaceFile",
            json!({"path": target, "old": "x", "new": "xy", "replace_all": true}),
        ))
        .await;
    assert!(amplified.is_error);
    assert_eq!(
        fs::read(&target).expect("read unchanged amplification fixture"),
        original
    );

    fs::write(&target, "x".repeat(MAX_TEXT_FILE_BYTES + 1)).expect("write oversized fixture");
    let oversized = dispatcher
        .dispatch(&ToolCall::new(
            "oversized",
            "StrReplaceFile",
            json!({"path": target, "old": "x", "new": "y"}),
        ))
        .await;
    assert!(oversized.is_error);
    assert_eq!(
        fs::metadata(&target).expect("stat oversized input").len(),
        (MAX_TEXT_FILE_BYTES + 1) as u64
    );
    fs::remove_dir_all(root).expect("remove string replacement test directory");
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
async fn dispatcher_reads_defaulted_and_explicit_line_pages_without_normalizing_content() {
    let root = test_root("read-pages");
    fs::create_dir_all(&root).expect("create read-pages test directory");
    let target = root.join("pages.txt");
    fs::write(&target, "first\r\nsecond\nthird\nfourth").expect("write page fixture");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());

    let first = dispatcher
        .dispatch(&ToolCall::new(
            "page-default-offset",
            "read_file",
            json!({"path": target, "limit": 2}),
        ))
        .await;
    assert!(!first.is_error, "{}", first.content);
    assert_eq!(
        first.content,
        "[read_file page offset=1 range=1-2]\nfirst\r\nsecond\n[next_offset=3]"
    );

    let middle = dispatcher
        .dispatch(&ToolCall::new(
            "page-middle",
            "read_file",
            json!({"path": target, "offset": 3, "limit": 1}),
        ))
        .await;
    assert!(!middle.is_error, "{}", middle.content);
    assert_eq!(
        middle.content,
        "[read_file page offset=3 range=3-3]\nthird\n[next_offset=4]"
    );

    let final_page = dispatcher
        .dispatch(&ToolCall::new(
            "page-final",
            "read_file",
            json!({"path": target, "offset": 4}),
        ))
        .await;
    assert!(!final_page.is_error, "{}", final_page.content);
    assert_eq!(
        final_page.content,
        "[read_file page offset=4 range=4-4]\nfourth\n[end_of_file]"
    );

    let after_end = dispatcher
        .dispatch(&ToolCall::new(
            "page-after-end",
            "read_file",
            json!({"path": target, "offset": 5, "limit": 1}),
        ))
        .await;
    assert!(!after_end.is_error, "{}", after_end.content);
    assert_eq!(
        after_end.content,
        "[read_file page offset=5 range=empty]\n[end_of_file]"
    );
    fs::remove_dir_all(root).expect("remove read-pages test directory");
}

#[tokio::test]
async fn dispatcher_marks_empty_and_boundary_limited_pages() {
    let root = test_root("read-page-boundary");
    fs::create_dir_all(&root).expect("create page-boundary test directory");
    let empty = root.join("empty.txt");
    fs::write(&empty, "").expect("write empty page fixture");
    let dispatcher = ToolDispatcher::new(ToolRegistry::new());
    let empty_page = dispatcher
        .dispatch(&ToolCall::new(
            "page-empty",
            "read_file",
            json!({"path": empty, "offset": 1}),
        ))
        .await;
    assert_eq!(
        empty_page.content,
        "[read_file page offset=1 range=empty]\n[end_of_file]"
    );

    let boundary = root.join("boundary.txt");
    let content = "x\n".repeat(2 * 1024 * 1024);
    fs::write(&boundary, format!("{content}after-boundary\n")).expect("write boundary fixture");
    let boundary_page = dispatcher
        .dispatch(&ToolCall::new(
            "page-boundary",
            "read_file",
            json!({"path": boundary, "offset": 2 * 1024 * 1024, "limit": 1}),
        ))
        .await;
    assert_eq!(
        boundary_page.content,
        "[read_file page offset=2097152 range=2097152-2097152]\nx\n[read_boundary_reached]"
    );
    fs::remove_dir_all(root).expect("remove page-boundary test directory");
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
