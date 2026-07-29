//! Queueing, rendering, and responsiveness terminal-client integration tests.

mod support;

use misy_core::{ModelId, ModelRef, ProviderId};
use misy_tui::{
    BrowserPlatform, TranscriptRow, TuiClient, UiKey, browser_command, validate_authorization_url,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    style::{Color, Modifier},
};
use serde_json::json;
use std::time::{Duration, Instant};
use support::tui::{
    RecordingBrowser, authorize_first_provider, buffer_lines, core_with_providers, render_buffer,
    select_first_model, test_client, wait_for,
};

#[tokio::test(flavor = "current_thread")]
async fn accepted_command_history_survives_a_new_client_and_restores_its_draft() {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "history-output.txt")]);
    let paths = misy_core::MisyPaths::from_root(temporary.path().join("misy"));
    core.select_model(ModelRef::new(
        ProviderId::new("fixture"),
        ModelId::new("fixture-model"),
    ))
    .await
    .expect("select fixture model");
    let mut first = misy_tui::TuiClient::with_persistent_history(
        core.clone(),
        RecordingBrowser::default(),
        &paths,
    )
    .await;
    first.insert_text("/provider");
    first.submit_composer().expect("accepted provider command");
    first.handle_key(UiKey::Escape).expect("close providers");
    first.insert_text("session-one");
    first.submit_composer().expect("accepted prompt");
    wait_for(&mut first, |client| {
        client.state().composer_input().is_empty()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-one"))
    })
    .await;
    drop(first);

    let mut second =
        misy_tui::TuiClient::with_persistent_history(core, RecordingBrowser::default(), &paths)
            .await;
    second.insert_text("draft");
    second.handle_key(UiKey::Up).expect("previous command");
    assert_eq!(second.state().composer_input(), "session-one");
    second.handle_key(UiKey::Up).expect("older command");
    assert_eq!(second.state().composer_input(), "/provider");
    second.handle_key(UiKey::Down).expect("newer prompt");
    assert_eq!(second.state().composer_input(), "session-one");
    second.handle_key(UiKey::Down).expect("restore draft");
    assert_eq!(second.state().composer_input(), "draft");
    second.handle_ctrl_c();
}

#[tokio::test(flavor = "current_thread")]
async fn renderer_shows_transcript_status_popup_and_cursor() {
    let (_temporary, mut client, _) = test_client().await;
    client
        .handle_input("/unknown")
        .expect("visible command error");
    client.insert_text("/");
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| misy_tui::render(frame, client.state()))
        .expect("render");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("model not selected"));
    assert!(rendered.contains("/provider"));
    assert!(rendered.contains("/model"));
}

#[tokio::test(flavor = "current_thread")]
async fn startup_header_scrolls_away_with_earlier_transcript_content() {
    let (_temporary, mut client, _) = test_client().await;
    let initial = buffer_lines(&render_buffer(client.state(), 72, 16), 72);
    assert!(initial.iter().any(|line| line.contains("Misy v")));
    assert!(initial.iter().any(|line| line.contains("Welcome back!")));
    select_first_model(&mut client).await;
    client.handle_input("burst").expect("submit burst");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text.len() == 4_096))
    })
    .await;

    let scrolled = buffer_lines(&render_buffer(client.state(), 72, 16), 72);
    assert!(!scrolled.iter().any(|line| line.contains("Misy v")));
    assert!(scrolled.iter().any(|line| line.contains("xxx")));
}

#[tokio::test(flavor = "current_thread")]
async fn renderer_matches_composer_popup_and_footer_layout() {
    let (_temporary, mut client, _) = test_client().await;
    let empty = render_buffer(client.state(), 60, 12);
    let empty_lines = buffer_lines(&empty, 60);
    let composer = empty_lines
        .iter()
        .position(|line| line.contains("> Ask anything, / for commands"))
        .expect("empty composer row");
    let footer = empty_lines
        .iter()
        .position(|line| line.starts_with("  ? for shortcuts"))
        .expect("footer row");
    assert!(empty_lines[composer - 1].contains('╭'));
    assert!(empty_lines[composer + 1].contains('╰'));
    assert_eq!(footer, empty_lines.len() - 1);
    assert!(empty_lines[footer].ends_with("model not selected"));
    client.insert_text("/mo");
    let popup = render_buffer(client.state(), 60, 12);
    let popup_lines = buffer_lines(&popup, 60);
    let composer = popup_lines
        .iter()
        .position(|line| line.contains("> /mo"))
        .expect("composer row");
    let suggestion = popup_lines
        .iter()
        .position(|line| line.contains("/model") && line.contains("Choose a model"))
        .expect("popup row");
    assert!(suggestion > composer);
    assert!(
        popup
            .content()
            .iter()
            .any(|cell| cell.bg == Color::DarkGray)
    );
    assert!(popup.content().iter().any(|cell| cell.fg == Color::Cyan));
    assert!(
        popup
            .content()
            .iter()
            .any(|cell| cell.modifier.contains(Modifier::DIM))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn provider_flow_stays_in_a_centered_popup() {
    let (_temporary, mut client, _) = test_client().await;
    authorize_first_provider(&mut client).await;
    client.handle_key(UiKey::Escape).expect("provider list");
    let buffer = render_buffer(client.state(), 160, 30);
    let lines = buffer_lines(&buffer, 160);
    let title = lines
        .iter()
        .position(|line| line.contains(" Providers"))
        .expect("provider title");
    let border = &lines[title.saturating_sub(1)];
    let left = border
        .chars()
        .position(|character| character == '┌')
        .expect("popup left border");
    let right = border
        .chars()
        .position(|character| character == '┐')
        .expect("popup right border");
    let terminal_width = border.chars().count();
    assert!(left > 0);
    assert!(right + 1 < terminal_width);
    assert!((left as isize - (terminal_width - right - 1) as isize).abs() <= 1);
    let row = lines
        .iter()
        .position(|line| line.contains("1.") && line.contains("Fixture AI"))
        .expect("numbered provider row");
    assert_eq!(row, title + 2);
    assert!(lines[row].contains("✓ authenticated"));
    assert!(lines.iter().any(|line| line.contains("enter open")));
    assert!(buffer.content().iter().any(|cell| cell.fg == Color::Green));

    client.handle_key(UiKey::Enter).expect("provider settings");
    let detail = buffer_lines(&render_buffer(client.state(), 160, 30), 160);
    assert!(detail.iter().any(|line| line.contains("Fixture AI")));
    assert!(detail.iter().any(|line| line.contains("Log out")));
    assert!(detail.iter().any(|line| line.contains("esc back")));
}

#[tokio::test(flavor = "current_thread")]
async fn narrow_provider_popup_separates_names_from_authentication_status() {
    let (_temporary, core) = core_with_providers(&[
        ("anthropic", "Anthropic", "anthropic"),
        ("kimi", "Kimi", "kimi"),
        ("openai", "OpenAI", "openai"),
    ]);
    for provider in ["kimi", "openai"] {
        core.complete_auth(
            &ProviderId::new(provider),
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("authenticate provider");
    }
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/provider").expect("providers");

    let lines = buffer_lines(&render_buffer(client.state(), 44, 12), 44);

    assert!(
        lines
            .iter()
            .any(|line| line.contains("Anthropic  not authenticated"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Kimi       ✓ authenticated"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("OpenAI     ✓ authenticated"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn renderer_shows_and_removes_the_single_line_busy_indicator() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("block-session").expect("start work");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_some()
    })
    .await;
    let busy = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(
        busy.iter()
            .any(|line| line.contains("Thinking…") && line.contains("esc to interrupt"))
    );
    client.handle_key(UiKey::Escape).expect("interrupt work");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    })
    .await;
    let idle = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(!idle.iter().any(|line| line.contains("Thinking…")));
}

#[tokio::test(flavor = "current_thread")]
async fn escape_closes_model_picker_without_interrupting_the_active_queue() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client
        .handle_input("block-session")
        .expect("start first prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_some()
    })
    .await;
    client
        .handle_input("session-two")
        .expect("queue second prompt");
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.pump_events();
    client.handle_input("/model").expect("open model picker");

    assert_eq!(client.state().mode(), misy_tui::UiMode::ModelList);
    client
        .handle_key(UiKey::Escape)
        .expect("close model picker");

    assert_eq!(client.state().mode(), misy_tui::UiMode::Input);
    assert!(client.state().active_submission().is_some());
    let queued = buffer_lines(&render_buffer(client.state(), 72, 16), 72);
    assert!(queued.iter().any(|line| line.contains("↳ session-two")));
    assert!(
        !client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled"))
    );

    client
        .handle_key(UiKey::Escape)
        .expect("cancel active prompt");
    client
        .handle_key(UiKey::Escape)
        .expect("repeat active cancellation");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "two"))
            && client.state().active_submission().is_none()
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_prompts_render_separately_and_run_in_fifo_order() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client
        .handle_input("block-session")
        .expect("start first prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_some()
    })
    .await;
    client
        .handle_input("session-two")
        .expect("queue second prompt");
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.pump_events();
    assert!(
        !client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-two"))
    );
    let queued = buffer_lines(&render_buffer(client.state(), 72, 16), 72);
    assert!(queued.iter().any(|line| line.contains("↳ session-two")));
    client
        .handle_key(UiKey::Escape)
        .expect("cancel first prompt");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "two"))
            && client.state().active_submission().is_none()
    })
    .await;
    let rows = client.state().transcript();
    let cancelled = rows
        .iter()
        .position(|row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled"))
        .expect("first prompt cancellation");
    let second = rows
        .iter()
        .position(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-two"))
        .expect("second prompt start");
    let response = rows
        .iter()
        .position(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "two"))
        .expect("second prompt response");
    assert!(cancelled < second && second < response);
}

#[tokio::test(flavor = "current_thread")]
async fn rapid_prompts_survive_event_result_interleaving_in_fifo_order() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    let prompts = [
        "block-session",
        "session-one",
        "session-one",
        "session-one",
        "session-one",
    ];
    for prompt in prompts {
        client.handle_input(prompt).expect("queue prompt");
    }

    // The preview and the transcript are projections of core acceptances, not of local input:
    // before the `SubmissionAccepted` events are pumped, neither shows the submitted prompts.
    let preview = buffer_lines(&render_buffer(client.state(), 72, 30), 72);
    assert!(preview.iter().all(|line| !line.contains('↳')));
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .all(|row| !matches!(row, TranscriptRow::UserPrompt(_)))
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    client.pump_events();
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "block-session"))
    );
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .all(|row| !matches!(row, TranscriptRow::UserPrompt(text) if text == "session-one"))
    );
    let queued = buffer_lines(&render_buffer(client.state(), 72, 30), 72);
    assert_eq!(
        queued
            .iter()
            .filter(|line| line.contains("↳ session-one"))
            .count(),
        3
    );
    assert!(queued.iter().any(|line| line.contains("more queued")));

    client
        .handle_key(UiKey::Escape)
        .expect("cancel blocking prompt");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .filter(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "one"))
            .count()
            == 4
            && client.state().active_submission().is_none()
    })
    .await;

    let rows = client.state().transcript();
    let user_prompts = rows
        .iter()
        .filter_map(|row| match row {
            TranscriptRow::UserPrompt(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(user_prompts, prompts);
    let assistant_messages = rows
        .iter()
        .filter_map(|row| match row {
            TranscriptRow::AssistantText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages, ["one", "one", "one", "one"]);
    assert_eq!(
        rows.iter()
            .filter(
                |row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled")
            )
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_escape_cancels_only_the_captured_active_turn() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client
        .handle_input("block-session")
        .expect("start first prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_some()
    })
    .await;
    client
        .handle_input("session-two")
        .expect("queue second prompt");
    client
        .handle_key(UiKey::Escape)
        .expect("cancel active prompt");
    client
        .handle_key(UiKey::Escape)
        .expect("repeat active cancellation");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "two"))
            && client.state().active_submission().is_none()
    })
    .await;
    let rendered = buffer_lines(&render_buffer(client.state(), 72, 16), 72);
    assert!(!rendered.iter().any(|line| line.contains("↳ session-two")));
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-two"))
    );
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter(
                |row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled")
            )
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn escape_cancels_the_core_head_when_tui_events_are_stale() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client
        .handle_input("stale-head-first")
        .expect("fast prompt");
    client
        .handle_input("stale-head-second")
        .expect("queued blocking prompt");
    tokio::time::sleep(Duration::from_millis(100)).await;
    client
        .handle_key(UiKey::Escape)
        .expect("cancel actual core head");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled"))
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn escape_cancels_only_the_active_head_and_keeps_the_queue() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    // The first two prompts use one-second fixture replies so each Escape lands while its
    // submission is still active; the fixture answers requests strictly in order, so the third
    // answer arrives only after both cancelled sleeps drain (~2s), well inside the deadline.
    client
        .handle_input("queue-drain-first")
        .expect("start first prompt");
    client
        .handle_input("queue-drain-second")
        .expect("queue second prompt");
    client
        .handle_input("queue-drain-third")
        .expect("queue third prompt");
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.pump_events();
    client
        .handle_key(UiKey::Escape)
        .expect("cancel first prompt");
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::UserPrompt(text) if text == "queue-drain-second"),
        )
    })
    .await;
    // Escape has no queue-clearing timeout, so an immediate second press must cancel only the
    // new head and leave the third prompt queued.
    client
        .handle_key(UiKey::Escape)
        .expect("cancel only the second prompt");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "two"))
            && client.state().active_submission().is_none()
    })
    .await;
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter(
                |row| matches!(row, TranscriptRow::Info(text) if text == "submission cancelled")
            )
            .count(),
        2
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authorization_urls_are_validated_and_windows_uses_direct_arguments() {
    let url = "https://example.test/authorize?client=misy&state=opaque";
    validate_authorization_url(url).expect("valid URL");
    assert!(validate_authorization_url("javascript:alert(1)").is_err());
    assert!(validate_authorization_url("https://example.test/a\nnext").is_err());
    let command = browser_command(BrowserPlatform::Windows, url).expect("browser command");
    assert_eq!(command.program, "rundll32");
    assert_eq!(command.args, ["url.dll,FileProtocolHandler", url]);
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_event_pump_keeps_ctrl_c_responsive() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client
        .handle_input("continuous-stream")
        .expect("continuous stream");
    client.pump_events();
    let backlog_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if client.pump_events() == 256 {
            break;
        }
        assert!(
            Instant::now() < backlog_deadline,
            "continuous stream did not fill one bounded event batch"
        );
    }
    let started = Instant::now();
    client.handle_ctrl_c();
    assert!(!client.state().should_exit());
    assert!(started.elapsed() < Duration::from_secs(1));
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    })
    .await;
    client.handle_input("/exit").expect("clean shutdown");
    assert!(client.state().should_exit());
}

#[tokio::test(flavor = "current_thread")]
async fn selected_model_remains_provider_scoped() {
    let model = ModelRef::new(ProviderId::new("openai"), ModelId::new("gpt-5"));
    assert_eq!(model.provider.as_str(), "openai");
    assert_eq!(model.model.as_str(), "gpt-5");
}

#[tokio::test(flavor = "current_thread")]
async fn model_event_refreshes_the_state_from_the_core_snapshot() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "models")]);
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("store credentials");
    let mut client = misy_tui::TuiClient::new(core.clone(), RecordingBrowser::default()).await;
    let expected = ModelRef::new(provider, ModelId::new("fixture-model"));
    core.select_model(expected.clone())
        .await
        .expect("select model");

    wait_for(&mut client, |client| {
        client.state().selected_model() == Some(expected.clone())
    })
    .await;

    assert_eq!(
        client.state().selected_model(),
        core.snapshot().selected_model
    );
}
