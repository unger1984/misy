//! Fan-out of events to lossy (bounded) and lossless (unbounded) subscribers.
//!
//! The core event bus and the provider host share this single delivery policy so the two layers
//! cannot diverge under load: a slow bounded listener drops the events it cannot keep up with
//! but stays subscribed, while a channel whose receiver is gone is pruned on the next emit.

use std::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Delivers cloned events to every live subscriber in subscription order.
#[derive(Debug)]
pub(crate) struct Fanout<T> {
    // Registration, snapshot replay, and emit share each lock, so a new subscriber never
    // observes a live event before the snapshot that establishes its initial state.
    lossy: Mutex<Vec<mpsc::Sender<T>>>,
    lossless: Mutex<Vec<UnboundedSender<T>>>,
}

impl<T> Default for Fanout<T> {
    fn default() -> Self {
        Self {
            lossy: Mutex::new(Vec::new()),
            lossless: Mutex::new(Vec::new()),
        }
    }
}

impl<T: Clone> Fanout<T> {
    /// Registers a bounded subscriber after replaying `snapshot` into its channel.
    ///
    /// A full channel drops the event instead of blocking the emitter; the subscriber stays
    /// registered until its receiver is dropped.
    pub(crate) fn subscribe(
        &self,
        capacity: usize,
        snapshot: impl IntoIterator<Item = T>,
    ) -> mpsc::Receiver<T> {
        let (sender, receiver) = mpsc::channel(capacity);
        let mut subscribers = self
            .lossy
            .lock()
            .expect("lossy fan-out mutex must not be poisoned");
        for event in snapshot {
            // Snapshot replay is best effort; a full channel still receives live events.
            let _ = sender.try_send(event);
        }
        subscribers.push(sender);
        receiver
    }

    /// Registers an unbounded subscriber after replaying `snapshot` into its channel.
    ///
    /// The unbounded queue never drops events; use it for lifecycle-sensitive consumers.
    pub(crate) fn subscribe_lossless(
        &self,
        snapshot: impl IntoIterator<Item = T>,
    ) -> UnboundedReceiver<T> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut subscribers = self
            .lossless
            .lock()
            .expect("lossless fan-out mutex must not be poisoned");
        for event in snapshot {
            // The receiver is returned alive below, so this send cannot fail.
            let _ = sender.send(event);
        }
        subscribers.push(sender);
        receiver
    }

    /// Delivers `event` to every subscriber, pruning channels whose receivers are gone.
    pub(crate) fn emit(&self, event: &T) {
        self.lossy
            .lock()
            .expect("lossy fan-out mutex must not be poisoned")
            .retain(|sender| match sender.try_send(event.clone()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => true,
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            });
        self.lossless
            .lock()
            .expect("lossless fan-out mutex must not be poisoned")
            .retain(|sender| sender.send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lossy_count(fanout: &Fanout<i32>) -> usize {
        fanout
            .lossy
            .lock()
            .expect("lossy fan-out mutex must not be poisoned")
            .len()
    }

    fn lossless_count(fanout: &Fanout<i32>) -> usize {
        fanout
            .lossless
            .lock()
            .expect("lossless fan-out mutex must not be poisoned")
            .len()
    }

    #[test]
    fn replays_snapshot_before_live_events() {
        let fanout = Fanout::default();
        let mut receiver = fanout.subscribe(4, [1, 2]);

        fanout.emit(&3);

        assert_eq!(receiver.try_recv(), Ok(1));
        assert_eq!(receiver.try_recv(), Ok(2));
        assert_eq!(receiver.try_recv(), Ok(3));
    }

    #[test]
    fn full_lossy_channel_drops_the_event_but_stays_subscribed() {
        let fanout = Fanout::default();
        let mut receiver = fanout.subscribe(1, []);

        fanout.emit(&1);
        fanout.emit(&2);

        assert_eq!(lossy_count(&fanout), 1);
        assert_eq!(receiver.try_recv(), Ok(1));
        assert!(receiver.try_recv().is_err());

        fanout.emit(&3);

        assert_eq!(receiver.try_recv(), Ok(3));
    }

    #[test]
    fn closed_lossy_channel_is_pruned_on_emit() {
        let fanout: Fanout<i32> = Fanout::default();
        let receiver = fanout.subscribe(1, []);
        drop(receiver);

        fanout.emit(&1);

        assert_eq!(lossy_count(&fanout), 0);
    }

    #[test]
    fn closed_lossless_channel_is_pruned_on_emit() {
        let fanout: Fanout<i32> = Fanout::default();
        let receiver = fanout.subscribe_lossless([]);
        drop(receiver);

        fanout.emit(&1);

        assert_eq!(lossless_count(&fanout), 0);
    }

    #[test]
    fn lossless_delivery_never_drops_events() {
        let fanout = Fanout::default();
        let mut receiver = fanout.subscribe_lossless([1, 2]);

        for event in 3..=10 {
            fanout.emit(&event);
        }

        for expected in 1..=10 {
            assert_eq!(receiver.try_recv(), Ok(expected));
        }
    }
}
