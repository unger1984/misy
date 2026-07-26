use super::{CoreEvent, MisyCore};
use crate::ProviderEvent;
use std::{
    sync::{
        Arc,
        mpsc::{Receiver, TrySendError},
    },
    thread,
};

impl MisyCore {
    pub(super) fn start_provider_event_router(&self, receiver: Receiver<ProviderEvent>) {
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if let Some(sender) = inner
                    .routes
                    .lock()
                    .expect("provider routes mutex must not be poisoned")
                    .get(event.provider.as_str())
                    .cloned()
                {
                    let _ = sender.send(event);
                }
            }
        });
    }

    pub(super) fn emit(&self, event: CoreEvent) {
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .expect("core subscribers mutex must not be poisoned");
        subscribers.retain(|sender| match sender.try_send(event.clone()) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
        let mut lossless_subscribers = self
            .inner
            .lossless_subscribers
            .lock()
            .expect("lossless core subscribers mutex must not be poisoned");
        lossless_subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }
}
