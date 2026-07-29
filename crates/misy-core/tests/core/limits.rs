//! Agent-loop limit tests.

use super::{fixture_model, receive_until, test_core};
use misy_core::{CoreEvent, Message};

#[tokio::test]
async fn core_stops_after_sixty_four_model_turns() {
    let (_temporary, core, _) = test_core("turn-limit");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("turn-limit"))
        .await
        .expect("submit");

    let received = receive_until(&mut events, submission, |event| {
        matches!(
            event,
            CoreEvent::Failed { submission: id, message }
                if *id == submission && message.contains("64")
        )
    })
    .await;
    assert!(
        received.iter().any(
            |event| matches!(event, CoreEvent::Failed { message, .. } if message.contains("64"))
        )
    );
    core.shutdown().await.expect("shutdown");
}
