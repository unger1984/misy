use super::Composer;
use misy_core::ImageAttachment;

fn test_image(red: u8) -> ImageAttachment {
    ImageAttachment::from_rgba(1, 1, vec![red, 0, 0, 255]).expect("valid test image")
}

#[test]
fn edits_unicode_at_the_cursor() {
    let mut composer = Composer::default();
    composer.insert_str("aйc");
    composer.move_left();
    composer.backspace();
    composer.insert_str("b");
    assert_eq!(composer.text(), "abc");
    composer.move_home();
    composer.delete();
    assert_eq!(composer.text(), "bc");
}

#[test]
fn slash_popup_tracks_edits_and_dismissal() {
    let mut composer = Composer::default();
    composer.insert_str("/");
    assert_eq!(composer.popup_rows().len(), 5);
    composer.insert_str("mo");
    assert_eq!(composer.selected_command(), Some("/model"));
    composer.dismiss_popup();
    assert!(!composer.popup_visible());
    composer.insert_str("d");
    assert!(composer.popup_visible());
}

#[test]
fn slash_popup_wraps_selection() {
    let mut composer = Composer::default();
    composer.insert_str("/");

    composer.popup_up();
    assert_eq!(composer.selected_command(), Some("/exit"));
    assert_eq!(
        composer.popup_rows(),
        [
            "  /provider  Configure provider authentication",
            "  /model  Choose a model",
            "  /status  Show provider usage and limits",
            "  /usage  Show provider usage and limits",
            "› /exit  Exit Misy",
        ]
    );

    composer.popup_down();
    assert_eq!(composer.selected_command(), Some("/provider"));
    assert!(composer.popup_rows()[0].starts_with("› /provider"));
}

#[test]
fn tab_completion_replaces_the_filter_and_hides_the_popup() {
    let mut composer = Composer::default();
    composer.insert_str("/mo");

    assert!(composer.complete_selected_command());
    assert_eq!(composer.text(), "/model ");
    assert_eq!(composer.cursor(), "/model ".len());
    assert!(!composer.popup_visible());
}

#[test]
fn seeded_history_is_bounded_and_collapses_adjacent_duplicates() {
    let history = (0..105)
        .map(|index| format!("prompt-{index}"))
        .chain(["prompt-104".to_owned()])
        .collect();
    let mut composer = Composer::with_history(history);

    assert_eq!(composer.history_len(), 100);
    composer.history_previous();
    assert_eq!(composer.text(), "prompt-104");
}

#[test]
fn mouse_position_maps_display_cells_to_unicode_boundaries() {
    let mut composer = Composer::default();
    composer.insert_str("a界b\nnext");

    composer.position_cursor(0, 2);
    composer.insert_str("!");
    assert_eq!(composer.text(), "a!界b\nnext");

    composer.position_cursor(1, 2);
    composer.insert_str("!");
    assert_eq!(composer.text(), "a!界b\nne!xt");

    composer.position_cursor(9, 0);
    composer.insert_str("!");
    assert_eq!(composer.text(), "a!界b\nne!xt!");
}

#[test]
fn image_tokens_are_bounded_and_removed_as_atomic_units() {
    let mut composer = Composer::default();
    for red in 0..4 {
        composer
            .insert_image(test_image(red))
            .expect("attach within limit");
    }
    assert_eq!(composer.attachment_count(), 4);
    assert_eq!(
        composer.insert_image(test_image(5)),
        Err("a prompt can contain at most 4 images")
    );
    assert!(composer.text().contains("[Image #4]"));

    composer.move_home();
    composer.delete();

    assert_eq!(composer.attachment_count(), 3);
    assert!(!composer.text().contains("[Image #4]"));
    assert_eq!(composer.text().matches("[Image #1]").count(), 1);

    composer.clear();
    composer.insert_image(test_image(1)).expect("attach image");
    composer.backspace();
    assert!(composer.text().is_empty());
    assert_eq!(composer.attachment_count(), 0);
}

#[test]
fn submitted_history_recalls_image_attachments_in_the_current_session() {
    let mut composer = Composer::default();
    composer.insert_image(test_image(1)).expect("attach image");
    composer.insert_str("describe this");
    assert_eq!(composer.history_text(), "describe this");
    assert_eq!(composer.draft().text, "describe this");
    composer.record_submitted_snapshot(composer.history_snapshot());

    composer.clear();
    composer.history_previous();

    assert_eq!(composer.text(), "[Image #1] describe this");
    assert_eq!(composer.attachment_count(), 1);
}

#[test]
fn submission_preserves_spacing_and_history_deduplication_ignores_cursor() {
    let mut composer = Composer::default();
    composer.insert_str("  preserve spacing  ");
    assert_eq!(composer.draft().text, "  preserve spacing  ");
    assert!(composer.record_submitted_snapshot(composer.history_snapshot()));

    composer.move_home();
    assert!(!composer.record_submitted_snapshot(composer.history_snapshot()));
    assert_eq!(composer.history_len(), 1);
}

#[test]
fn submitted_history_keeps_only_the_latest_image_payload() {
    let mut composer = Composer::default();
    composer.insert_image(test_image(1)).expect("attach image");
    composer.insert_str("first");
    composer.record_submitted_snapshot(composer.history_snapshot());

    composer.clear();
    composer.insert_image(test_image(2)).expect("attach image");
    composer.insert_str("second");
    composer.record_submitted_snapshot(composer.history_snapshot());

    composer.clear();
    composer.history_previous();
    assert_eq!(composer.attachment_count(), 1);
    composer.history_previous();
    assert_eq!(composer.text(), "first");
    assert_eq!(composer.attachment_count(), 0);
}

#[test]
fn history_navigation_restores_the_attachment_bearing_draft() {
    let mut composer = Composer::default();
    composer.record_submitted("older prompt");
    composer.insert_image(test_image(1)).expect("attach image");
    composer.insert_str("draft");

    composer.history_previous();
    assert_eq!(composer.text(), "older prompt");
    composer.history_next();

    assert_eq!(composer.text(), "[Image #1] draft");
    assert_eq!(composer.attachment_count(), 1);
}
