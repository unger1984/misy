//! Serialized model polling and PTY input for published command sessions.

use super::{ActivityInput, ActivityManager, ActivityRecord};
use crate::{ActivityId, ActivityOutput};
use std::{io::Write, sync::atomic::Ordering, time::Duration};
use tokio::sync::watch;

#[derive(Debug)]
pub(in crate::tools) enum InteractionError {
    Unknown,
    StdinClosed,
    Write(String),
    Cancelled,
}

impl ActivityManager {
    pub(in crate::tools) async fn interact(
        &self,
        id: ActivityId,
        chars: &str,
        wait: Duration,
        max_output_tokens: usize,
        mut cancellation: Option<watch::Receiver<bool>>,
    ) -> Result<ActivityOutput, InteractionError> {
        let record = self.record(id).ok_or(InteractionError::Unknown)?;
        let _interaction = record.interaction.lock().await;
        if !record.session_open.load(Ordering::Acquire) {
            return Err(InteractionError::Unknown);
        }
        if !chars.is_empty() {
            record.write_input(chars)?;
        }
        wait_for_output(&record, wait, &mut cancellation).await?;
        let output = record.model_output(max_output_tokens);
        if output.activity.status.is_terminal() {
            record.session_open.store(false, Ordering::Release);
        }
        Ok(output)
    }
}

async fn wait_for_output(
    record: &ActivityRecord,
    wait: Duration,
    cancellation: &mut Option<watch::Receiver<bool>>,
) -> Result<(), InteractionError> {
    let changed = record.changed.notified();
    tokio::pin!(changed);
    let mut poll_cancel = record.poll_cancel.subscribe();
    if record.summary().status.is_terminal() || wait.is_zero() {
        return Ok(());
    }
    tokio::select! {
        () = tokio::time::sleep(wait) => Ok(()),
        () = &mut changed => Ok(()),
        _ = poll_cancel.changed() => Ok(()),
        () = wait_for_cancellation(cancellation) => Err(InteractionError::Cancelled),
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

impl ActivityRecord {
    fn write_input(&self, chars: &str) -> Result<(), InteractionError> {
        let mut input = self
            .input
            .lock()
            .expect("activity input mutex must not be poisoned");
        let ActivityInput::Pty(writer) = &mut *input else {
            return Err(InteractionError::StdinClosed);
        };
        writer
            .write_all(chars.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| InteractionError::Write(error.to_string()))
    }
}
