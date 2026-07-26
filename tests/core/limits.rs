//! Agent-loop limit tests.

use super::{fixture_model, receive_until, test_core};
use misy::{CoreEvent, Message};

#[test]
fn core_stops_after_sixty_four_model_turns() {
    let (_temporary, core, _) = test_core("turn-limit");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core.submit(Message::user("turn-limit")).expect("submit");

    let received = receive_until(&events, submission, |event| {
        matches!(
            event,
            CoreEvent::Failed { submission: id, message }
                if *id == submission && message.contains("64")
        )
    });
    assert!(
        received.iter().any(
            |event| matches!(event, CoreEvent::Failed { message, .. } if message.contains("64"))
        )
    );
    core.shutdown().expect("shutdown");
}
