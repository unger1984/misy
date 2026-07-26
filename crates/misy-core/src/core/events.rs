//! Fan-out of normalized core events to lossy and lossless client subscriptions.

use super::{CoreEvent, CoreState};
use crate::ProviderEvent;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{
    runtime::Handle,
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
};

pub(crate) struct EventSubscribers {
    lossy: Mutex<Vec<mpsc::Sender<CoreEvent>>>,
    lossless: Mutex<Vec<UnboundedSender<CoreEvent>>>,
}

impl Default for EventSubscribers {
    fn default() -> Self {
        Self {
            lossy: Mutex::new(Vec::new()),
            lossless: Mutex::new(Vec::new()),
        }
    }
}

impl EventSubscribers {
    pub(super) fn subscribe(
        &self,
        capacity: usize,
        snapshot: impl IntoIterator<Item = CoreEvent>,
    ) -> mpsc::Receiver<CoreEvent> {
        let (sender, receiver) = mpsc::channel(capacity);
        // Registration and the snapshot share this lock so a listener never observes a live
        // event before the discovery snapshot that establishes its initial state.
        let mut subscribers = self
            .lossy
            .lock()
            .expect("lossy core subscribers mutex must not be poisoned");
        for event in snapshot {
            let _ = sender.try_send(event);
        }
        subscribers.push(sender);
        receiver
    }

    pub(super) fn subscribe_lossless(
        &self,
        snapshot: impl IntoIterator<Item = CoreEvent>,
    ) -> UnboundedReceiver<CoreEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut subscribers = self
            .lossless
            .lock()
            .expect("lossless core subscribers mutex must not be poisoned");
        for event in snapshot {
            let _ = sender.send(event);
        }
        subscribers.push(sender);
        receiver
    }

    pub(super) fn emit(&self, event: &CoreEvent) {
        self.lossy
            .lock()
            .expect("lossy core subscribers mutex must not be poisoned")
            .retain(|sender| match sender.try_send(event.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
        self.lossless
            .lock()
            .expect("lossless core subscribers mutex must not be poisoned")
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

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
                .get(event.provider.as_str())
                .cloned();
            if let Some(route) = route {
                let _ = route.send(event);
            }
        }
    });
}

pub(super) type ProviderRoutes = Mutex<BTreeMap<String, UnboundedSender<ProviderEvent>>>;
