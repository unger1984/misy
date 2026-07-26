use super::{CoreEvent, MisyCore};
use crate::ProviderEvent;
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender, TrySendError},
    },
    thread,
};

#[derive(Default)]
pub(super) struct LosslessSubscribers {
    senders: Mutex<Vec<Sender<CoreEvent>>>,
}

impl LosslessSubscribers {
    pub(super) fn subscribe(
        &self,
        snapshot: impl IntoIterator<Item = CoreEvent>,
    ) -> Receiver<CoreEvent> {
        let mut senders = self
            .senders
            .lock()
            .expect("lossless core subscribers mutex must not be poisoned");
        let (sender, receiver) = mpsc::channel();
        for event in snapshot {
            let _ = sender.send(event);
        }
        senders.push(sender);
        receiver
    }

    pub(super) fn emit(&self, event: &CoreEvent) {
        self.senders
            .lock()
            .expect("lossless core subscribers mutex must not be poisoned")
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

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

    pub(super) fn emit(&self, event: &CoreEvent) {
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .expect("core subscribers mutex must not be poisoned");
        subscribers.retain(|sender| match sender.try_send(event.clone()) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
        self.inner.lossless_subscribers.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::LosslessSubscribers;
    use crate::{CoreEvent, ProviderId};
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    #[test]
    fn lossless_registration_serializes_snapshot_before_concurrent_events() {
        let subscribers = Arc::new(LosslessSubscribers::default());
        let (snapshot_started_tx, snapshot_started_rx) = mpsc::channel();
        let (release_snapshot_tx, release_snapshot_rx) = mpsc::channel();
        let snapshot = std::iter::once_with(move || {
            snapshot_started_tx.send(()).expect("snapshot started");
            release_snapshot_rx.recv().expect("release snapshot");
            CoreEvent::ProviderDiscovered {
                provider: ProviderId::new("fixture"),
            }
        });
        let subscribing = {
            let subscribers = Arc::clone(&subscribers);
            thread::spawn(move || subscribers.subscribe(snapshot))
        };
        snapshot_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("subscription holds registration lock");
        let (emitting, emit_finished) = {
            let subscribers = Arc::clone(&subscribers);
            let (started_tx, started_rx) = mpsc::channel();
            let (finished_tx, finished_rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                started_tx.send(()).expect("emitter started");
                subscribers.emit(&CoreEvent::AuthenticationChanged {
                    provider: ProviderId::new("fixture"),
                    authenticated: true,
                });
                finished_tx.send(()).expect("emitter finished");
            });
            started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("emitter started");
            assert!(
                finished_rx.recv_timeout(Duration::from_millis(50)).is_err(),
                "live emit must wait until snapshot registration completes"
            );
            (handle, finished_rx)
        };
        release_snapshot_tx.send(()).expect("release subscription");
        let events = subscribing.join().expect("subscription thread");
        emitting.join().expect("emitter thread");
        emit_finished
            .recv_timeout(Duration::from_secs(1))
            .expect("emitter finished after registration");

        assert!(matches!(
            events
                .recv_timeout(Duration::from_secs(1))
                .expect("snapshot"),
            CoreEvent::ProviderDiscovered { .. }
        ));
        assert!(matches!(
            events
                .recv_timeout(Duration::from_secs(1))
                .expect("live event"),
            CoreEvent::AuthenticationChanged {
                authenticated: true,
                ..
            }
        ));
    }
}
