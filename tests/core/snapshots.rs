//! Subscription-snapshot coverage for the headless core.

use super::test_core;
use misy::CoreEvent;
use std::time::Duration;

#[test]
fn subscription_replays_the_discovered_provider_snapshot() {
    let (_temporary, core, _) = test_core("provider-snapshot");
    let events = core.subscribe();
    assert!(matches!(
        events
            .recv_timeout(Duration::from_secs(1))
            .expect("provider snapshot"),
        CoreEvent::ProviderDiscovered { provider } if provider.as_str() == "fixture"
    ));
    core.shutdown().expect("shutdown");
}
