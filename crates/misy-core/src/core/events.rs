//! Fan-out of normalized core events to lossy and lossless client subscriptions.

use super::{CoreEvent, CoreState};
use crate::{ProviderId, fanout::Fanout, providers::ProviderEvent};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

/// Core-event delivery with the shared fan-out policy.
pub(crate) type EventSubscribers = Fanout<CoreEvent>;

pub(super) fn start_provider_event_router(
    runtime: &Handle,
    state: Arc<CoreState>,
    mut receiver: UnboundedReceiver<ProviderEvent>,
) {
    runtime.spawn(async move {
        while let Some(event) = receiver.recv().await {
            let route = state
                .routes
                .lock()
                .expect("provider routes mutex must not be poisoned")
                .get(&event.provider)
                .cloned();
            if let Some(route) = route {
                // A closed route means the provider process is gone; its late events are moot.
                let _ = route.send(event);
            }
        }
    });
}

pub(super) type ProviderRoutes = Mutex<BTreeMap<ProviderId, UnboundedSender<ProviderEvent>>>;
