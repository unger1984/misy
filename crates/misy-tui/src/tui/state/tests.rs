use super::{ActiveView, ProviderAction, ProviderChoice, UiState};
use crate::tui::action::UiAction;
use misy_core::{
    ActivityOutput, ActivitySummary, CoreEvent, ProviderAuthMethod, ProviderDisplayName, ProviderId,
};
use std::time::{Duration, Instant};

fn activity(id: u64, kind: &str, status: &str, title: &str) -> ActivitySummary {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "kind": kind,
        "status": status,
        "title": title,
        "cwd": null,
        "started_at_ms": id,
        "exit_code": null
    }))
    .expect("deserialize activity fixture")
}

fn provider_choice(id: &str, display_name: &str, authenticated: bool) -> ProviderChoice {
    ProviderChoice {
        id: ProviderId::new(id),
        display_name: ProviderDisplayName::new(display_name),
        authenticated,
        credential_method: authenticated.then(|| "oauth".to_owned()),
        auth_methods: vec![ProviderAuthMethod {
            id: "oauth".to_owned(),
            display_name: "Browser OAuth".to_owned(),
        }],
    }
}

fn question_request(id: u64, text: &str) -> misy_core::QuestionRequest {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "tool_call_id": format!("call-{id}"),
        "source": {"Submission": id},
        "questions": [{
            "question": text,
            "header": "",
            "options": [
                {"label": "A", "description": ""},
                {"label": "B", "description": ""}
            ],
            "multi_select": false
        }]
    }))
    .expect("question fixture")
}

#[test]
fn question_snapshot_recovers_requests_in_fifo_order_without_a_modal() {
    let mut state = UiState::default();
    state.composer.insert_str("saved draft");
    let first = question_request(1, "First question");
    let second = question_request(2, "Second question");
    let mut snapshot = state.snapshot.clone();
    snapshot.pending_questions = vec![first, second.clone()];
    state.apply_snapshot(snapshot);
    assert_eq!(state.mode(), crate::tui::UiMode::Question);
    assert!(state.modal_presentation(8).is_none());
    assert_eq!(
        state
            .question_presentation(8)
            .expect("question surface")
            .title,
        "First question"
    );
    assert_eq!(state.composer_input(), "saved draft");

    let mut snapshot = state.snapshot.clone();
    snapshot.pending_questions = vec![second];
    state.apply_snapshot(snapshot);
    assert_eq!(
        state
            .question_presentation(8)
            .expect("question surface")
            .title,
        "Second question"
    );
}

#[test]
fn pending_question_hides_the_generation_busy_label() {
    let mut state = UiState::default();
    let now = Instant::now();
    state.submission_started_at = Some(now - Duration::from_secs(3));
    assert!(state.busy_label(now).is_some());

    let mut snapshot = state.snapshot.clone();
    snapshot.pending_questions = vec![question_request(1, "Choose")];
    state.apply_snapshot(snapshot);

    assert!(state.busy_label(now).is_none());
}

fn refreshed_choices() -> Vec<ProviderChoice> {
    vec![
        provider_choice("fixture", "Fixture AI", true),
        provider_choice("fixture-two", "Second AI", false),
    ]
}

#[test]
fn refresh_providers_keeps_picker_filter_and_selection_while_updating_rows() {
    let mut state = UiState::default();
    state.open_providers(vec![
        provider_choice("fixture", "Fixture AI", false),
        provider_choice("fixture-two", "Second AI", false),
    ]);
    state.insert_filter("second");

    state.refresh_providers(refreshed_choices());

    let Some(ActiveView::Providers(view)) = &state.view else {
        panic!("provider picker must stay open");
    };
    assert_eq!(view.labels(), ["Second AI"]);

    for _ in 0.."second".len() {
        state.backspace_filter();
    }
    state.reduce(&UiAction::PickerDown);
    state.refresh_providers(refreshed_choices());

    let Some(ActiveView::Providers(view)) = &state.view else {
        panic!("provider picker must stay open");
    };
    assert_eq!(view.labels(), ["Fixture AI", "Second AI"]);
    assert_eq!(view.selected_value(), Some(&ProviderId::new("fixture-two")));
    let rows = view.visible_rows(8);
    assert_eq!(
        rows.first().and_then(|row| row.description.as_deref()),
        Some("✓ authenticated")
    );
    assert!(rows.last().is_some_and(|row| row.selected));
}

#[test]
fn refresh_providers_updates_settings_rows_without_rebuilding_the_view() {
    let mut state = UiState::default();
    state.open_providers(vec![provider_choice("fixture", "Fixture AI", false)]);
    state.open_provider_settings(&ProviderId::new("fixture"));
    assert_eq!(
        state.selected_provider_action(),
        Some((
            ProviderId::new("fixture"),
            ProviderAction::Authorize("oauth".to_owned())
        ))
    );

    state.refresh_providers(vec![provider_choice("fixture", "Fixture AI", true)]);

    let Some(ActiveView::ProviderSettings {
        credential_method,
        actions,
        ..
    }) = &state.view
    else {
        panic!("provider settings must stay open");
    };
    assert_eq!(credential_method.as_deref(), Some("oauth"));
    assert_eq!(actions.labels(), ["Log out"]);
    assert_eq!(
        state.selected_provider_action(),
        Some((ProviderId::new("fixture"), ProviderAction::Logout))
    );
}

#[test]
fn selected_running_task_can_be_stopped_directly_from_activity_list() {
    let mut state = UiState::default();
    state.snapshot.activities.push(
        serde_json::from_value(serde_json::json!({
            "id": 7,
            "kind": "task",
            "status": "running",
            "title": "fixture task",
            "cwd": null,
            "started_at_ms": 1,
            "exit_code": null
        }))
        .expect("deserialize activity fixture"),
    );
    state.open_activities();
    state.reduce(&UiAction::PickerDown);

    assert_eq!(
        state.selected_activity_to_stop().map(|id| id.get()),
        Some(7)
    );
}

#[test]
fn activity_snapshot_refresh_preserves_filter_and_selection() {
    let mut state = UiState::default();
    state.snapshot.activities = vec![
        activity(1, "task", "running", "compile workspace"),
        activity(2, "task", "running", "watch tests"),
    ];
    let watched_id = state.snapshot.activities[1].id;
    state.open_activities();
    state.insert_filter("watch");

    let mut snapshot = state.snapshot.clone();
    snapshot.activities = vec![
        activity(1, "task", "completed", "compile workspace"),
        activity(2, "task", "completed", "watch tests"),
    ];
    state.apply_snapshot(snapshot);

    assert_eq!(state.picker_labels(), ["● watch tests"]);
    assert_eq!(
        state.selected_activity_choice(),
        Some(crate::tui::activity_picker::ActivityChoice::Activity(
            watched_id
        ))
    );
}

#[test]
fn activity_bar_remains_visible_for_recent_terminal_tasks() {
    let mut state = UiState::default();
    state.snapshot.activities = vec![
        activity(1, "task", "running", "compile"),
        activity(2, "task", "waiting", "test"),
        activity(3, "task", "completed", "lint"),
        activity(4, "agent", "running", "review"),
    ];

    assert!(state.activity_bar_visible());
    assert_eq!(state.activity_counts(), (2, 1, 1));
    assert_eq!(
        state.activity_bar_label(),
        "  ↓ (2 running tasks · 1 completed · 1 agents)"
    );

    state.snapshot.activities = vec![activity(3, "task", "completed", "lint")];
    assert!(state.activity_bar_visible());
    assert_eq!(state.activity_counts(), (0, 1, 0));
}

fn output_with_fragments(id: u64, lines: &str) -> ActivityOutput {
    serde_json::from_value(serde_json::json!({
        "activity": {
            "id": id,
            "kind": "task",
            "status": "running",
            "title": "fixture task",
            "cwd": null,
            "started_at_ms": 1,
            "exit_code": null
        },
        "stdout": lines,
        "stderr": "warning\n",
        "stdout_truncated": false,
        "stderr_truncated": false,
        "fragments": [
            {"stream": "stdout", "text": lines},
            {"stream": "stderr", "text": "warning\n"}
        ],
        "message": null
    }))
    .expect("deserialize ordered output fixture")
}

#[test]
fn activity_popup_embeds_a_bounded_tail_preview() {
    let mut state = UiState::default();
    state.snapshot.activities = vec![activity(7, "task", "running", "fixture task")];
    state.open_activities();
    state.reduce(&UiAction::PickerDown);
    state.set_activity_output(output_with_fragments(
        7,
        "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
    ));

    let modal = state.modal_presentation(20).expect("activities popup");
    let labels = modal
        .rows
        .iter()
        .map(|row| row.label.as_str())
        .collect::<Vec<_>>();
    assert!(
        labels
            .iter()
            .any(|label| label.starts_with("Preview · task-7"))
    );
    assert!(!labels.contains(&"one"));
    assert!(labels.contains(&"seven"));
    assert!(labels.contains(&"stderr: warning"));
}

#[test]
fn activity_log_escape_restores_the_preserved_picker() {
    let mut state = UiState::default();
    state.snapshot.activities = vec![
        activity(1, "task", "running", "compile workspace"),
        activity(2, "task", "running", "watch tests"),
    ];
    state.open_activities();
    state.insert_filter("watch");
    let watched_id = state.snapshot.activities[1].id;
    state.open_activity_detail(watched_id);
    assert_eq!(state.mode(), crate::tui::UiMode::ActivityDetail);

    state.reduce(&UiAction::PickerBack);

    assert_eq!(state.mode(), crate::tui::UiMode::ActivityList);
    assert_eq!(state.picker_labels(), ["○ watch tests"]);
    assert_eq!(
        state.selected_activity_choice(),
        Some(crate::tui::activity_picker::ActivityChoice::Activity(
            watched_id
        ))
    );
}

#[test]
fn activity_log_scrolls_to_earlier_output_and_returns_to_follow_tail() {
    let mut state = UiState::default();
    state.snapshot.activities = vec![activity(7, "task", "running", "fixture task")];
    state.open_activities();
    state.reduce(&UiAction::PickerDown);
    let id = state.snapshot.activities[0].id;
    state.open_activity_detail(id);
    let lines = (1..=30)
        .map(|line| format!("line-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.set_activity_output(output_with_fragments(7, &lines));
    assert!(
        state
            .activity_log_lines(5)
            .iter()
            .any(|line| line == "line-30")
    );

    state.scroll_activity_log_up(true);
    assert!(
        !state
            .activity_log_lines(5)
            .iter()
            .any(|line| line == "line-30")
    );
    state.scroll_activity_log_down(true);
    assert!(
        state
            .activity_log_lines(5)
            .iter()
            .any(|line| line == "line-30")
    );
}

#[test]
fn ordered_output_labels_keep_stderr_at_its_capture_position() {
    let output: ActivityOutput = serde_json::from_value(serde_json::json!({
        "activity": {
            "id": 1, "kind": "task", "status": "completed", "title": "ordered",
            "cwd": null, "started_at_ms": 1, "exit_code": 0
        },
        "stdout": "beforeafter", "stderr": "warning", "stdout_truncated": false,
        "stderr_truncated": false,
        "fragments": [
            {"stream": "stdout", "text": "before"},
            {"stream": "stderr", "text": "warning"},
            {"stream": "stdout", "text": "after"}
        ],
        "message": null
    }))
    .expect("deserialize interleaved output");

    assert_eq!(
        super::activities::output_labels(&output),
        [
            "status: Completed · exit 0",
            "before",
            "stderr: warning",
            "after"
        ]
    );
}

#[test]
fn terminal_activity_event_adds_one_complete_transcript_item() {
    let mut state = UiState::default();
    let output: ActivityOutput = serde_json::from_value(serde_json::json!({
        "activity": {
            "id": 9,
            "kind": "task",
            "status": "completed",
            "title": "compile workspace",
            "cwd": null,
            "started_at_ms": 1,
            "exit_code": 0
        },
        "stdout": "done\n",
        "stderr": "",
        "stdout_truncated": false,
        "stderr_truncated": false,
        "message": null
    }))
    .expect("deserialize activity output fixture");

    state.apply_core_event(CoreEvent::ActivityFinished {
        output: output.clone(),
    });

    assert_eq!(
        state.transcript(),
        [super::TranscriptRow::ActivityFinished(output)]
    );
}

#[test]
fn background_agent_finished_event_adds_compact_notice() {
    let mut state = UiState::default();
    let agent = serde_json::from_value(serde_json::json!({
        "id": 4,
        "activity_id": 9,
        "title": "Check cleanup",
        "model": {"provider": "fixture", "model": "model-a"},
        "status": "completed",
        "run_in_background": true,
        "started_at_ms": 1,
        "finished_at_ms": 2,
        "terminal_message": "clean"
    }))
    .expect("deserialize agent summary");
    state.apply_core_event(CoreEvent::AgentFinished {
        agent,
        result: "clean".to_owned(),
    });
    assert!(matches!(
        state.transcript(),
        [super::TranscriptRow::Info(message)]
            if message == "agent-4 · Check cleanup · Completed · clean"
    ));
}
