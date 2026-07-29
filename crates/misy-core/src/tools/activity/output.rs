//! Ordered, bounded command output capture and model-delivery projections.

use crate::{ActivityOutputFragment, ActivityOutputStream};
use std::collections::VecDeque;

pub(super) const OUTPUT_MAX_BYTES: usize = 1024 * 1024;
pub(in crate::tools) const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;
const BYTES_PER_TOKEN: usize = 4;
const HALF_BUDGET: usize = OUTPUT_MAX_BYTES / 2;
const FRAGMENT_OVERHEAD: usize = 32;
const MAX_FRAGMENTS_PER_HALF: usize = 2_048;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct DeliveryCursor {
    position: u64,
    stdout: u64,
    stderr: u64,
}

#[derive(Debug)]
struct CapturedFragment {
    stream: ActivityOutputStream,
    start: u64,
    bytes: Vec<u8>,
}

impl CapturedFragment {
    fn end(&self) -> u64 {
        self.start
            .saturating_add(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))
    }

    fn cost(&self) -> usize {
        self.bytes.len().saturating_add(FRAGMENT_OVERHEAD)
    }
}

#[derive(Debug, Default)]
pub(super) struct OrderedOutput {
    head: VecDeque<CapturedFragment>,
    tail: VecDeque<CapturedFragment>,
    head_cost: usize,
    tail_cost: usize,
    total: u64,
    stdout_total: u64,
    stderr_total: u64,
    stdout_incomplete: bool,
    stderr_incomplete: bool,
}

#[derive(Debug)]
pub(super) struct OutputProjection {
    pub(super) fragments: Vec<ActivityOutputFragment>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) stdout_truncated: bool,
    pub(super) stderr_truncated: bool,
    pub(super) cursor: DeliveryCursor,
}

impl OrderedOutput {
    pub(super) fn append(&mut self, stream: ActivityOutputStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let start = self.total;
        self.total = self
            .total
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        match stream {
            ActivityOutputStream::Stdout => {
                self.stdout_total = self
                    .stdout_total
                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            }
            ActivityOutputStream::Stderr => {
                self.stderr_total = self
                    .stderr_total
                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            }
            ActivityOutputStream::System => return,
        }
        let head_kept = self.append_head(stream, start, bytes);
        self.append_tail(
            stream,
            start.saturating_add(u64::try_from(head_kept).unwrap_or(u64::MAX)),
            &bytes[head_kept..],
        );
    }

    pub(super) fn mark_incomplete(&mut self, stream: ActivityOutputStream) {
        match stream {
            ActivityOutputStream::Stdout => self.stdout_incomplete = true,
            ActivityOutputStream::Stderr => self.stderr_incomplete = true,
            ActivityOutputStream::System => {}
        }
    }

    pub(super) fn snapshot(&self) -> OutputProjection {
        self.project(DeliveryCursor::default(), usize::MAX)
    }

    pub(super) fn deliver(
        &self,
        cursor: DeliveryCursor,
        max_output_tokens: usize,
    ) -> OutputProjection {
        self.project(cursor, max_output_tokens.saturating_mul(BYTES_PER_TOKEN))
    }

    fn append_head(&mut self, stream: ActivityOutputStream, start: u64, bytes: &[u8]) -> usize {
        let same_stream = self
            .head
            .back()
            .is_some_and(|fragment| fragment.stream == stream && fragment.end() == start);
        if !same_stream && self.head.len() == MAX_FRAGMENTS_PER_HALF {
            return 0;
        }
        let overhead = if same_stream { 0 } else { FRAGMENT_OVERHEAD };
        let available = HALF_BUDGET
            .saturating_sub(self.head_cost)
            .saturating_sub(overhead);
        let kept = available.min(bytes.len());
        if kept == 0 {
            return 0;
        }
        if same_stream {
            if let Some(fragment) = self.head.back_mut() {
                fragment.bytes.extend_from_slice(&bytes[..kept]);
            }
        } else {
            self.head.push_back(CapturedFragment {
                stream,
                start,
                bytes: bytes[..kept].to_vec(),
            });
        }
        self.head_cost = self.head_cost.saturating_add(overhead).saturating_add(kept);
        kept
    }

    fn append_tail(&mut self, stream: ActivityOutputStream, start: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let same_stream = self
            .tail
            .back()
            .is_some_and(|fragment| fragment.stream == stream && fragment.end() == start);
        if same_stream {
            if let Some(fragment) = self.tail.back_mut() {
                fragment.bytes.extend_from_slice(bytes);
            }
            self.tail_cost = self.tail_cost.saturating_add(bytes.len());
        } else {
            let fragment = CapturedFragment {
                stream,
                start,
                bytes: bytes.to_vec(),
            };
            self.tail_cost = self.tail_cost.saturating_add(fragment.cost());
            self.tail.push_back(fragment);
        }
        self.trim_tail();
    }

    fn trim_tail(&mut self) {
        while self.tail_cost > HALF_BUDGET || self.tail.len() > MAX_FRAGMENTS_PER_HALF {
            let fragment_count = self.tail.len();
            let Some(front) = self.tail.front_mut() else {
                return;
            };
            let excess = self.tail_cost.saturating_sub(HALF_BUDGET).max(1);
            if fragment_count <= MAX_FRAGMENTS_PER_HALF && excess < front.bytes.len() {
                front.bytes.drain(..excess);
                front.start = front
                    .start
                    .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
                self.tail_cost = self.tail_cost.saturating_sub(excess);
                continue;
            }
            let removed = self.tail.pop_front().expect("tail front must exist");
            self.tail_cost = self.tail_cost.saturating_sub(removed.cost());
        }
    }

    fn project(&self, cursor: DeliveryCursor, byte_budget: usize) -> OutputProjection {
        let available = self.fragments_after(cursor.position);
        let available_bytes: usize = available.iter().map(|fragment| fragment.bytes.len()).sum();
        let (head, tail) = select_head_tail(available, byte_budget);
        let kept_stdout = stream_bytes(&head, &tail, ActivityOutputStream::Stdout);
        let kept_stderr = stream_bytes(&head, &tail, ActivityOutputStream::Stderr);
        let delta_stdout = self.stdout_total.saturating_sub(cursor.stdout);
        let delta_stderr = self.stderr_total.saturating_sub(cursor.stderr);
        let omitted_stdout = delta_stdout.saturating_sub(kept_stdout);
        let omitted_stderr = delta_stderr.saturating_sub(kept_stderr);
        let omitted = omitted_stdout.saturating_add(omitted_stderr);
        let fragments = public_fragments(&head, &tail, omitted);
        let stdout = stream_text(&head, &tail, ActivityOutputStream::Stdout, omitted_stdout);
        let stderr = stream_text(&head, &tail, ActivityOutputStream::Stderr, omitted_stderr);
        debug_assert!(
            available_bytes <= OUTPUT_MAX_BYTES,
            "retained output must stay within its common budget"
        );
        OutputProjection {
            fragments,
            stdout,
            stderr,
            stdout_truncated: omitted_stdout > 0 || self.stdout_incomplete,
            stderr_truncated: omitted_stderr > 0 || self.stderr_incomplete,
            cursor: DeliveryCursor {
                position: self.total,
                stdout: self.stdout_total,
                stderr: self.stderr_total,
            },
        }
    }

    fn fragments_after(&self, position: u64) -> Vec<CapturedFragment> {
        self.head
            .iter()
            .chain(&self.tail)
            .filter_map(|fragment| slice_after(fragment, position))
            .collect()
    }
}

fn slice_after(fragment: &CapturedFragment, position: u64) -> Option<CapturedFragment> {
    if fragment.end() <= position {
        return None;
    }
    let offset = usize::try_from(position.saturating_sub(fragment.start)).unwrap_or(usize::MAX);
    let offset = offset.min(fragment.bytes.len());
    Some(CapturedFragment {
        stream: fragment.stream,
        start: fragment
            .start
            .saturating_add(u64::try_from(offset).unwrap_or(u64::MAX)),
        bytes: fragment.bytes[offset..].to_vec(),
    })
}

fn select_head_tail(
    fragments: Vec<CapturedFragment>,
    byte_budget: usize,
) -> (Vec<CapturedFragment>, Vec<CapturedFragment>) {
    let total: usize = fragments.iter().map(|fragment| fragment.bytes.len()).sum();
    if total <= byte_budget {
        return (fragments, Vec::new());
    }
    let head_budget = byte_budget / 2;
    let tail_budget = byte_budget.saturating_sub(head_budget);
    (
        take_head(&fragments, head_budget),
        take_tail(&fragments, tail_budget),
    )
}

fn take_head(fragments: &[CapturedFragment], mut budget: usize) -> Vec<CapturedFragment> {
    let mut kept = Vec::new();
    for fragment in fragments {
        if budget == 0 {
            break;
        }
        let length = budget.min(fragment.bytes.len());
        kept.push(CapturedFragment {
            stream: fragment.stream,
            start: fragment.start,
            bytes: fragment.bytes[..length].to_vec(),
        });
        budget -= length;
    }
    kept
}

fn take_tail(fragments: &[CapturedFragment], mut budget: usize) -> Vec<CapturedFragment> {
    let mut kept = VecDeque::new();
    for fragment in fragments.iter().rev() {
        if budget == 0 {
            break;
        }
        let length = budget.min(fragment.bytes.len());
        let offset = fragment.bytes.len() - length;
        kept.push_front(CapturedFragment {
            stream: fragment.stream,
            start: fragment
                .start
                .saturating_add(u64::try_from(offset).unwrap_or(u64::MAX)),
            bytes: fragment.bytes[offset..].to_vec(),
        });
        budget -= length;
    }
    kept.into()
}

fn stream_bytes(
    head: &[CapturedFragment],
    tail: &[CapturedFragment],
    stream: ActivityOutputStream,
) -> u64 {
    head.iter()
        .chain(tail)
        .filter(|fragment| fragment.stream == stream)
        .map(|fragment| u64::try_from(fragment.bytes.len()).unwrap_or(u64::MAX))
        .fold(0, u64::saturating_add)
}

fn public_fragments(
    head: &[CapturedFragment],
    tail: &[CapturedFragment],
    omitted: u64,
) -> Vec<ActivityOutputFragment> {
    let mut fragments = head.iter().map(public_fragment).collect::<Vec<_>>();
    if omitted > 0 {
        fragments.push(ActivityOutputFragment {
            stream: ActivityOutputStream::System,
            text: omission_marker(omitted),
        });
    }
    fragments.extend(tail.iter().map(public_fragment));
    coalesce_public(fragments)
}

fn public_fragment(fragment: &CapturedFragment) -> ActivityOutputFragment {
    ActivityOutputFragment {
        stream: fragment.stream,
        text: String::from_utf8_lossy(&fragment.bytes).into_owned(),
    }
}

fn coalesce_public(fragments: Vec<ActivityOutputFragment>) -> Vec<ActivityOutputFragment> {
    let mut coalesced: Vec<ActivityOutputFragment> = Vec::new();
    for fragment in fragments {
        if let Some(previous) = coalesced.last_mut()
            && previous.stream == fragment.stream
        {
            previous.text.push_str(&fragment.text);
        } else {
            coalesced.push(fragment);
        }
    }
    coalesced
}

fn stream_text(
    head: &[CapturedFragment],
    tail: &[CapturedFragment],
    stream: ActivityOutputStream,
    omitted: u64,
) -> String {
    let mut bytes = Vec::new();
    for fragment in head.iter().filter(|fragment| fragment.stream == stream) {
        bytes.extend_from_slice(&fragment.bytes);
    }
    if omitted > 0 {
        bytes.extend_from_slice(omission_marker(omitted).as_bytes());
    }
    for fragment in tail.iter().filter(|fragment| fragment.stream == stream) {
        bytes.extend_from_slice(&fragment.bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn omission_marker(omitted: u64) -> String {
    format!("\n[... {omitted} bytes omitted ...]\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_budget_keeps_head_tail_and_exact_omission_count() {
        let mut output = OrderedOutput::default();
        let stdout = vec![b'a'; OUTPUT_MAX_BYTES];
        let stderr = vec![b'b'; OUTPUT_MAX_BYTES];
        output.append(ActivityOutputStream::Stdout, &stdout);
        output.append(ActivityOutputStream::Stderr, &stderr);

        let snapshot = output.snapshot();

        assert!(snapshot.stdout_truncated);
        assert!(snapshot.stderr_truncated);
        assert!(snapshot.fragments.iter().any(|fragment| {
            fragment.stream == ActivityOutputStream::System
                && fragment.text.contains("1048640 bytes omitted")
        }));
        let retained: usize = snapshot
            .fragments
            .iter()
            .filter(|fragment| fragment.stream != ActivityOutputStream::System)
            .map(|fragment| fragment.text.len())
            .sum();
        assert!(retained < OUTPUT_MAX_BYTES);
    }

    #[test]
    fn alternating_streams_stay_within_data_and_metadata_budget() {
        let mut output = OrderedOutput::default();
        for index in 0..100_000 {
            let stream = if index % 2 == 0 {
                ActivityOutputStream::Stdout
            } else {
                ActivityOutputStream::Stderr
            };
            output.append(stream, b"x");
        }

        assert!(output.head_cost + output.tail_cost <= OUTPUT_MAX_BYTES);
        assert!(output.head.len() + output.tail.len() <= MAX_FRAGMENTS_PER_HALF * 2);
    }

    #[test]
    fn model_budget_does_not_shrink_client_snapshot_and_advances_cursor() {
        let mut output = OrderedOutput::default();
        output.append(ActivityOutputStream::Stdout, &[b'x'; 100]);

        let delivered = output.deliver(DeliveryCursor::default(), 10);
        let client = output.snapshot();
        let next = output.deliver(delivered.cursor, 10);

        assert!(delivered.stdout.contains("60 bytes omitted"));
        assert_eq!(client.stdout, "x".repeat(100));
        assert!(next.stdout.is_empty());
    }

    #[test]
    fn fragments_preserve_observed_capture_order() {
        let mut output = OrderedOutput::default();
        output.append(ActivityOutputStream::Stdout, b"one");
        output.append(ActivityOutputStream::Stderr, b"two");
        output.append(ActivityOutputStream::Stdout, b"three");

        let streams = output
            .snapshot()
            .fragments
            .into_iter()
            .map(|fragment| (fragment.stream, fragment.text))
            .collect::<Vec<_>>();
        assert_eq!(
            streams,
            [
                (ActivityOutputStream::Stdout, "one".to_owned()),
                (ActivityOutputStream::Stderr, "two".to_owned()),
                (ActivityOutputStream::Stdout, "three".to_owned()),
            ]
        );
    }
}
