//! Model picker and popup integration tests.

mod support;

use misy_core::{ModelId, ModelRef, ProviderId};
use misy_tui::{TranscriptRow, TuiClient, UiKey, UiMode};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use serde_json::json;
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use support::tui::{
    RecordingBrowser, authorize_first_provider, buffer_lines, core_with_providers, render_buffer,
    select_first_model, test_client, wait_for,
};

async fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(path.exists(), "fixture did not create {}", path.display());
}

#[tokio::test(flavor = "current_thread")]
async fn model_picker_shows_partial_results_and_skips_unconfigured_provider() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Working AI", "good-models"),
        ("failed-provider", "Broken AI", "bad-models"),
        ("unconfigured", "Unused AI", "unused-models"),
    ]);
    for provider in ["fixture", "failed-provider"] {
        core.complete_auth(
            &ProviderId::new(provider),
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("store credentials");
    }
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 3
    })
    .await;
    let labels = client.state().picker_labels();
    assert!(labels.iter().any(|label| label == "fixture-model  128k"));
    assert!(
        labels
            .iter()
            .any(|label| label.contains("Broken AI — error"))
    );
    assert_eq!(client.running_provider_count().await, 2);
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_picker_opens_from_cached_models_before_the_refresh_arrives() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "good-models")]);
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    core.available_models().await.expect("populate model cache");

    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open cached models");

    assert_eq!(
        client.state().picker_labels(),
        ["fixture-model  128k", "fixture-model-b  64k"]
    );
    assert_eq!(
        client.state().picker_tabs(),
        [("All".to_owned(), true), ("Fixture AI".to_owned(), false)]
    );
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_picker_tabs_filter_models_and_preserve_filter_after_switching() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "First AI", "good-models"),
        ("fixture-two", "Second AI", "good-models"),
    ]);
    for provider in [ProviderId::new("fixture"), ProviderId::new("fixture-two")] {
        core.complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("store credentials");
    }
    core.available_models().await.expect("populate model cache");

    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open models");
    assert_eq!(client.state().picker_labels().len(), 4);
    client.insert_text("fixture-model-b");
    client.handle_key(UiKey::Right).expect("first provider tab");
    assert_eq!(client.state().picker_labels(), ["fixture-model-b  64k"]);
    assert_eq!(
        client.state().picker_tabs()[1],
        ("First AI".to_owned(), true)
    );
    client
        .handle_key(UiKey::Right)
        .expect("second provider tab");
    assert_eq!(client.state().picker_labels(), ["fixture-model-b  64k"]);
    client.handle_key(UiKey::Left).expect("first provider tab");
    assert_eq!(
        client.state().picker_tabs()[1],
        ("First AI".to_owned(), true)
    );
    thread::sleep(Duration::from_millis(50));
    client.pump_events();
    assert_eq!(
        client.state().picker_tabs()[1],
        ("First AI".to_owned(), true)
    );
    assert_eq!(client.state().picker_labels(), ["fixture-model-b  64k"]);
    client
        .handle_key(UiKey::Enter)
        .expect("confirm refreshed selection");
    wait_for(&mut client, |client| client.state().mode() == UiMode::Input).await;
    assert_eq!(
        client.state().selected_model().map(|model| model.provider),
        Some(ProviderId::new("fixture"))
    );
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn empty_cached_model_picker_renders_a_spinner_then_the_refreshed_models() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "slow-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open models");

    let lines = buffer_lines(&render_buffer(client.state(), 72, 18), 72);
    assert!(lines.iter().any(|line| line.contains("Loading models…")));
    assert!(lines.iter().any(|line| line.contains("Select model")));
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    })
    .await;
    assert!(
        client
            .state()
            .picker_labels()
            .iter()
            .any(|label| label.contains("fixture-model"))
    );
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_picker_uses_the_herdr_style_popup_without_clipping_on_narrow_terminals() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("/model").expect("open cached models");

    let buffer = render_buffer(client.state(), 72, 18);
    let lines = buffer_lines(&buffer, 72);
    assert!(lines.iter().any(|line| line.contains("Select model")));
    assert!(lines.iter().any(|line| line.contains("─")));
    assert!(lines.iter().any(|line| line.contains("↑↓ select")));
    assert!(lines.iter().any(|line| line.contains("›")));
    assert!(
        buffer
            .content()
            .iter()
            .any(|cell| cell.symbol() == "A" && cell.bg == Color::Blue)
    );

    let narrow = render_buffer(client.state(), 12, 8);
    assert_eq!(narrow.area.width, 12);
    assert_eq!(narrow.area.height, 8);
    let narrow_lines = buffer_lines(&narrow, 12);
    assert!(narrow_lines.iter().any(|line| line.contains("Select")));
    assert!(narrow_lines.iter().any(|line| line.contains("›")));
    assert!(narrow_lines.iter().any(|line| line.contains("esc")));
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn context_command_opens_a_scrollable_content_specific_popup() {
    let (_temporary, mut client, _) = test_client().await;
    client.handle_input("/context").expect("open context");
    assert_eq!(client.state().mode(), UiMode::Context);

    let lines = buffer_lines(&render_buffer(client.state(), 82, 24), 82);
    assert!(lines.iter().any(|line| line.contains("Context usage")));
    assert!(lines.iter().any(|line| line.contains("Estimated")));
    assert!(lines.iter().any(|line| line.contains("AGENTS.md")));
    client.handle_key(UiKey::End).expect("context end");
    let end_lines = buffer_lines(&render_buffer(client.state(), 82, 24), 82);
    assert!(
        end_lines
            .iter()
            .any(|line| line.contains("token estimate unavailable"))
    );
    client.handle_key(UiKey::PageUp).expect("context page up");
    client.handle_key(UiKey::Escape).expect("close context");
    assert_eq!(client.state().mode(), UiMode::Input);
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_popup_is_centered_and_capped_on_a_wide_terminal() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "good-models")]);
    populate_cached_models(&core, &["fixture"]).await;
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open cached models");

    let buffer = render_buffer(client.state(), 160, 30);
    let lines = buffer_lines(&buffer, 160);
    let border = lines
        .iter()
        .find(|line| line.contains('┌') && line.contains('┐'))
        .expect("popup top border");
    let left = border
        .chars()
        .position(|character| character == '┌')
        .expect("left popup border");
    let right = border
        .chars()
        .enumerate()
        .filter(|(_, character)| *character == '┐')
        .map(|(index, _)| index)
        .last()
        .expect("right popup border");
    let terminal_width = border.chars().count();

    assert!(
        left > 0,
        "popup should not start at the screen edge: {border:?}"
    );
    assert!(
        right + 1 < terminal_width,
        "popup should not end at the screen edge: {border:?}"
    );
    // The cap mirrors MAX_POPUP_WIDTH in tui/model_popup.rs (66 before the resize fix).
    assert!(
        right - left < 100,
        "popup should have a readable maximum width"
    );
    assert!((left as isize - (terminal_width - right - 1) as isize).abs() <= 1);
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_popup_wraps_provider_tabs_into_multiple_rows() {
    let provider_ids = [
        "provider-01",
        "provider-02",
        "provider-03",
        "provider-04",
        "provider-05",
        "provider-06",
        "provider-07",
    ];
    let specifications = provider_ids
        .iter()
        .map(|id| (*id, format!("Provider {id}"), "good-models"))
        .collect::<Vec<_>>();
    let specification_refs = specifications
        .iter()
        .map(|(id, name, target)| (*id, name.as_str(), *target))
        .collect::<Vec<_>>();
    let (_temporary, core) = core_with_providers(&specification_refs);
    populate_cached_models(&core, &provider_ids).await;
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open cached models");

    let lines = buffer_lines(&render_buffer(client.state(), 64, 24), 64);
    let tab_rows = lines
        .iter()
        .filter(|line| line.contains("Provider provider-"))
        .count();
    assert!(
        tab_rows >= 2,
        "provider tabs should wrap instead of clipping: {lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Provider provider-07"))
    );
    for _ in 0..provider_ids.len() {
        client
            .handle_key(UiKey::Right)
            .expect("switch provider tab");
    }
    let short_lines = buffer_lines(&render_buffer(client.state(), 64, 8), 64);
    assert!(
        short_lines
            .iter()
            .any(|line| line.contains("Provider provider-07")),
        "the active tab must stay visible on short screens: {short_lines:#?}"
    );
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_popup_scrolls_models_within_its_actual_content_height() {
    let provider_ids = [
        "provider-01",
        "provider-02",
        "provider-03",
        "provider-04",
        "provider-05",
        "provider-06",
    ];
    let specifications = provider_ids
        .iter()
        .map(|id| (*id, format!("Provider {id}"), "good-models"))
        .collect::<Vec<_>>();
    let specification_refs = specifications
        .iter()
        .map(|(id, name, target)| (*id, name.as_str(), *target))
        .collect::<Vec<_>>();
    let (_temporary, core) = core_with_providers(&specification_refs);
    populate_cached_models(&core, &provider_ids).await;
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("open cached models");
    for _ in 0..11 {
        client
            .handle_key(UiKey::Down)
            .expect("move model selection");
    }

    let lines = buffer_lines(&render_buffer(client.state(), 72, 12), 72);
    assert!(
        lines.iter().any(|line| line.contains('›')),
        "the selected model must remain visible in the popup viewport: {lines:#?}"
    );
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn closing_model_popup_redraws_the_underlying_screen_without_ghost_cells() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "good-models")]);
    populate_cached_models(&core, &["fixture"]).await;
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.insert_text("draft remains visible");
    let mut terminal = Terminal::new(TestBackend::new(72, 18)).expect("terminal");
    terminal
        .draw(|frame| misy_tui::render(frame, client.state()))
        .expect("draw underlying screen");
    let underlying = terminal.backend().buffer().clone();

    client.handle_input("/model").expect("open cached models");
    terminal
        .draw(|frame| misy_tui::render(frame, client.state()))
        .expect("draw popup");
    client.handle_key(UiKey::Escape).expect("close popup");
    terminal
        .draw(|frame| misy_tui::render(frame, client.state()))
        .expect("redraw underlying screen");

    assert_eq!(terminal.backend().buffer(), &underlying);
    client.handle_ctrl_c();
}

async fn populate_cached_models(core: &misy_core::MisyCore, provider_ids: &[&str]) {
    for provider_id in provider_ids {
        core.complete_auth(
            &ProviderId::new(*provider_id),
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("store credentials");
    }
    core.available_models().await.expect("populate model cache");
}

#[tokio::test(flavor = "current_thread")]
async fn reopened_model_picker_ignores_the_first_refresh_generation() {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "gated-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    client.handle_input("/model").expect("start first refresh");
    let target = temporary.path().join("gated-models");
    wait_for_path(&target.with_extension("first-started")).await;
    client.handle_key(UiKey::Escape).expect("close picker");
    client.handle_input("/model").expect("start second refresh");
    fs::write(
        target.with_extension("first-release"),
        "release first refresh",
    )
    .expect("release first refresh");
    wait_for_path(&target.with_extension("second-started")).await;
    client.pump_events();

    assert_eq!(client.state().mode(), UiMode::ModelList);
    assert_eq!(client.state().picker_labels(), ["Loading models…"]);
    fs::write(
        target.with_extension("second-release"),
        "release second refresh",
    )
    .expect("release second refresh");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    })
    .await;
    client.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn model_picker_filters_and_confirms_with_a_transcript_message() {
    let (_temporary, mut client, _) = test_client().await;
    authorize_first_provider(&mut client).await;
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("composer");
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    })
    .await;
    client.insert_text("model-b");
    assert_eq!(client.state().picker_labels().len(), 1);
    client
        .handle_key(UiKey::Enter)
        .expect("select filtered model");
    wait_for(&mut client, |client| client.state().mode() == UiMode::Input).await;
    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Info(message) if message == "model: fixture-model-b")
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn model_selection_cannot_be_abandoned_while_it_is_persisting() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "slow-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    })
    .await;
    client.handle_key(UiKey::Enter).expect("start selection");
    client.handle_key(UiKey::Escape).expect("escape is ignored");
    assert_eq!(client.state().mode(), UiMode::ModelList);
    wait_for(&mut client, |client| {
        client.state().selected_model().is_some()
    })
    .await;
    assert_eq!(
        client.state().selected_model(),
        Some(ModelRef::new(
            ProviderId::new("fixture"),
            ModelId::new("fixture-model")
        ))
    );
    assert!(
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Info(message) if message.starts_with("model:"))
        )
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_model_loading_ignores_its_late_error() {
    let (temporary, core) = core_with_providers(&[("fixture", "Broken AI", "slow-bad-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    let mut client = TuiClient::new(core.clone(), RecordingBrowser::default()).await;
    client.handle_input("/model").expect("models");
    client.handle_key(UiKey::Escape).expect("cancel loading");

    let target = temporary.path().join("slow-bad-models");
    wait_for_path(&target.with_extension("responded")).await;
    // The fixture answers requests strictly in order, so a finished second call proves
    // the late error was already read and handed to the runtime the client runs on.
    core.available_models().await.expect("delivery probe");
    for _ in 0..10 {
        client.pump_events();
        tokio::task::yield_now().await;
    }
    client.pump_events();

    assert_eq!(client.state().mode(), UiMode::Input);
    assert!(client.state().transcript().is_empty());
}
