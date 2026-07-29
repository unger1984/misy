//! Multimodal submission behavior exercised through the public core contract.

use super::*;

async fn image_core(name: &str) -> (tempfile::TempDir, MisyCore) {
    let (temporary, core, _) = test_core(name);
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate fixture");
    core.list_models(&provider).await.expect("list models");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    (temporary, core)
}

fn pixel() -> ImageAttachment {
    ImageAttachment::from_rgba(1, 1, vec![255, 0, 0, 255]).expect("encode image")
}

#[tokio::test]
async fn submits_images_only_to_capable_provider_models() {
    let (_temporary, core) = image_core("image-input").await;
    let mut events = core.subscribe_lossless();

    assert!(core.selected_model_supports(InputModality::Image));
    let submission = core
        .submit_with_attachments(Message::user("image-input"), vec![pixel()])
        .await
        .expect("submit image");
    let events = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;

    assert!(
        events.iter().any(|event| {
            matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "seen")
        }),
        "events: {events:?}"
    );
    assert!(
        core.history()
            .await
            .iter()
            .all(|entry| entry.attachments.is_empty())
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn rejects_images_for_a_text_only_model_before_acceptance() {
    let (_temporary, core) = image_core("image-rejected").await;
    let text_model = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));
    core.select_model(text_model)
        .await
        .expect("select text model");

    assert!(!core.selected_model_supports(InputModality::Image));
    let error = core
        .submit_with_attachments(Message::user("must reject"), vec![pixel()])
        .await
        .expect_err("text model must reject image");

    assert!(error.to_string().contains("does not support Image input"));
    assert!(core.history().await.is_empty());
    core.shutdown().await.expect("shutdown");
}
