//! Deterministic rendering checks for the fullscreen client surfaces.

use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn question_snapshot() -> misy_core::CoreSnapshot {
    let mut snapshot = UiState::default().snapshot;
    snapshot.todos = serde_json::from_value(serde_json::json!([
        {"title": "Plan the change", "status": "pending"},
        {"title": "Render the question", "status": "in_progress"},
        {"title": "Keep the draft", "status": "done"}
    ]))
    .expect("todo snapshot fixture");
    snapshot.pending_questions = serde_json::from_value(serde_json::json!([{
        "id": 1,
        "tool_call_id": "call-1",
        "source": {"Submission": 1},
        "questions": [{
            "question": "Which path should Misy take?",
            "header": "Decision",
            "options": [
                {"label": "Inline", "description": "Use the bottom slot"},
                {"label": "Modal", "description": "Use a popup"}
            ],
            "multi_select": false
        }]
    }]))
    .expect("question snapshot fixture");
    snapshot
}

fn multi_question_snapshot() -> misy_core::CoreSnapshot {
    let mut snapshot = question_snapshot();
    let second = &snapshot.pending_questions[0].questions[0];
    let mut second = second.clone();
    second.question = "Which demonstration?".to_owned();
    second.header = "Demo".to_owned();
    snapshot.pending_questions[0].questions.push(second);
    snapshot
}

fn rendered_screen(state: &UiState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, state))
        .expect("render frame");
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

fn rendered_lines(state: &UiState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render(frame, state))
        .expect("render frame");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

#[test]
fn list_line_truncates_both_columns_without_removing_the_gap() {
    let row = ListRowDisplay {
        number: 1,
        label: "Provider with a very long display name".to_owned(),
        context: None,
        description: Some("authentication status with extra details".to_owned()),
        selected: true,
        current: false,
    };

    let line = list_line(&row, 32, text_width(&row.label));
    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(line.width() <= 32);
    assert!(rendered.contains("…  authentica…"), "{rendered:?}");
}

#[test]
fn transcript_content_has_a_two_column_inset() {
    let line = inset_transcript_line(Line::raw("● Read file"));
    let rendered = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(rendered, "  ● Read file");
}

#[test]
fn question_replaces_the_composer_and_keeps_sticky_todos_visible() {
    let mut state = UiState::default();
    state.composer.insert_str("hidden draft");
    state.apply_snapshot(question_snapshot());

    assert!(state.modal_presentation(MAX_VIEW_ROWS).is_none());
    let lines = rendered_lines(&state, 80, 16);
    let screen = lines.join("\n");
    assert!(screen.contains("Which path should Misy take?"));
    assert!(!screen.contains("hidden draft"));
    assert!(screen.contains("  ☑ 1/3 Tasks"));
    for title in ["Plan the change", "Render the question", "Keep the draft"] {
        let line = lines
            .iter()
            .find(|line| line.contains(title))
            .expect("todo row");
        assert!(line.starts_with("  "), "{line:?}");
    }
}

#[test]
fn multiple_questions_end_on_a_separate_review_and_submit_tab() {
    let mut state = UiState::default();
    state.apply_snapshot(multi_question_snapshot());
    assert!(state.confirm_question().is_none());
    assert!(state.confirm_question().is_none());

    assert_eq!(question_height(&state, 20), 8);
    let screen = rendered_screen(&state, 80, 16);
    assert!(screen.contains("Review your answers before submit"));
    assert!(screen.contains("Submit"));
}

#[test]
fn short_question_surface_reserves_one_visible_choice_row() {
    assert_eq!(
        super::super::bottom_surface::question_rows(Rect::new(0, 0, 40, 4), false),
        1
    );
}

#[test]
fn short_question_surface_keeps_the_selected_option_visible_before_todos() {
    let mut state = UiState::default();
    state.apply_snapshot(question_snapshot());

    let screen = rendered_screen(&state, 80, 8);
    assert!(screen.contains("Inline"), "{screen:?}");
    assert!(screen.contains("○ Plan the change"), "{screen:?}");
}

#[test]
fn multiline_composer_is_budgeted_before_sticky_todos() {
    let mut state = UiState::default();
    state
        .composer
        .insert_str("line one\nline two\nline three\nline four\nline five");
    let mut snapshot = question_snapshot();
    snapshot.pending_questions.clear();
    state.apply_snapshot(snapshot);

    let screen = rendered_screen(&state, 80, 10);
    assert!(screen.contains("line five"), "{screen:?}");
    assert!(screen.contains("○ Plan the change"), "{screen:?}");
}

#[test]
fn clearing_the_todo_snapshot_hides_the_pinned_rows() {
    let mut state = UiState::default();
    state.apply_snapshot(question_snapshot());
    let mut cleared = state.snapshot.clone();
    cleared.todos.clear();
    state.apply_snapshot(cleared);

    let screen = rendered_screen(&state, 80, 16);
    assert!(!screen.contains("Plan the change"));
    assert_eq!(super::super::bottom_surface::todo_height(&state, 12), 0);
}

#[test]
fn todo_height_includes_the_kimi_style_header() {
    let mut state = UiState::default();
    state.apply_snapshot(question_snapshot());

    assert_eq!(super::super::bottom_surface::todo_height(&state, 12), 4);
}
