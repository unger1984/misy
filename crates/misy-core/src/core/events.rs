//! Fan-out of normalized core events to lossy and lossless client subscriptions.

use super::{CoreEvent, CoreState};
use crate::{fanout::Fanout, providers::ProviderEvent};
use std::sync::Arc;
use tokio::{runtime::Handle, sync::mpsc::UnboundedReceiver};

/// Core-event delivery with the shared fan-out policy.
pub(crate) type EventSubscribers = Fanout<CoreEvent>;

pub(super) fn start_provider_event_router(
    runtime: &Handle,
    _state: Arc<CoreState>,
    mut receiver: UnboundedReceiver<ProviderEvent>,
) {
    runtime.spawn(async move { while receiver.recv().await.is_some() {} });
}
