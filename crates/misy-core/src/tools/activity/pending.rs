//! Foreground waiting, cancellation, and publication of an existing process.

use super::{ActivityManager, ActivityRecord, StopReason};
use crate::ActivityOutput;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    time::{Instant, sleep_until},
};

pub(in crate::tools) struct PendingCommand {
    pub(in crate::tools::activity) manager: ActivityManager,
    pub(in crate::tools::activity) record: Arc<ActivityRecord>,
    pub(in crate::tools::activity) promoted: bool,
}

impl PendingCommand {
    pub(in crate::tools) async fn wait_or_promote(
        mut self,
        yield_after: Duration,
        mut cancellation: Option<watch::Receiver<bool>>,
        max_output_tokens: usize,
    ) -> ActivityOutput {
        if self.promoted {
            return self.record.model_output(max_output_tokens);
        }
        let deadline = Instant::now() + yield_after;
        loop {
            let record = Arc::clone(&self.record);
            let changed = record.changed.notified();
            tokio::pin!(changed);
            if record.summary().status.is_terminal() {
                return record.model_output(max_output_tokens);
            }
            tokio::select! {
                () = sleep_until(deadline) => {
                    if let Err(message) = self.promote() {
                        self.record.stop.send_replace(Some(StopReason::Requested));
                        wait_until_terminal(&self.record).await;
                        return self.record.output_with_message(message);
                    }
                    return self.record.model_output(max_output_tokens);
                }
                () = &mut changed => {}
                () = wait_for_cancellation(&mut cancellation) => {
                    record.stop.send_replace(Some(StopReason::Requested));
                    wait_until_terminal(&record).await;
                    return record.model_output(max_output_tokens);
                }
            }
        }
    }

    pub(in crate::tools::activity) fn promote(&mut self) -> Result<(), String> {
        self.manager.publish(&self.record)?;
        self.promoted = true;
        Ok(())
    }
}

async fn wait_for_cancellation(cancellation: &mut Option<watch::Receiver<bool>>) {
    let Some(cancellation) = cancellation else {
        return std::future::pending::<()>().await;
    };
    loop {
        if *cancellation.borrow() || cancellation.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_until_terminal(record: &ActivityRecord) {
    loop {
        let changed = record.changed.notified();
        tokio::pin!(changed);
        if record.summary().status.is_terminal() {
            return;
        }
        changed.await;
    }
}

impl Drop for PendingCommand {
    fn drop(&mut self) {
        if !self.promoted && !self.record.summary().status.is_terminal() {
            self.record.stop.send_replace(Some(StopReason::Requested));
        }
    }
}
