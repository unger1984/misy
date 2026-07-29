//! Optional, best-effort reporting of this client to a Herdr pane.

use misy_core::CoreSnapshot;
use std::{future::Future, pin::Pin, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    process::Command,
    sync::{mpsc, oneshot},
    time::timeout,
};

const AGENT_LABEL: &str = "misy";
const SOURCE: &str = "misy:cli";
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);

/// A state understood by Herdr's pane lifecycle API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HerdrAgentState {
    /// No core-owned work currently needs user attention.
    Idle,
    /// Misy has active or queued work.
    Working,
    /// Misy is waiting for an answer to a structured question.
    Blocked,
}

impl HerdrAgentState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
        }
    }
}

fn herdr_state(snapshot: &CoreSnapshot) -> HerdrAgentState {
    state_from_flags(
        !snapshot.pending_questions.is_empty(),
        snapshot.active_submission.is_some() || !snapshot.queued_submissions.is_empty(),
    )
}

fn state_from_flags(has_pending_questions: bool, has_work: bool) -> HerdrAgentState {
    if has_pending_questions {
        HerdrAgentState::Blocked
    } else if has_work {
        HerdrAgentState::Working
    } else {
        HerdrAgentState::Idle
    }
}

trait HerdrEnvironment {
    /// Returns an environment value, when present.
    fn value(&self, name: &str) -> Option<String>;
}

struct ProcessEnvironment;

impl HerdrEnvironment for ProcessEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HerdrConfiguration {
    executable: String,
    pane_id: String,
}

impl HerdrConfiguration {
    fn from_environment(environment: &impl HerdrEnvironment) -> Option<Self> {
        let herdr_environment = environment.value("HERDR_ENV")?;
        let pane_id = environment.value("HERDR_PANE_ID")?;
        if herdr_environment != "1" || pane_id.trim().is_empty() {
            return None;
        }
        let executable = environment
            .value("HERDR_BIN_PATH")
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| "herdr".to_owned());
        Some(Self {
            executable,
            pane_id,
        })
    }

    fn report_arguments(&self, state: HerdrAgentState) -> Vec<String> {
        vec![
            "pane".to_owned(),
            "report-agent".to_owned(),
            "--source".to_owned(),
            SOURCE.to_owned(),
            "--agent".to_owned(),
            AGENT_LABEL.to_owned(),
            "--state".to_owned(),
            state.as_str().to_owned(),
            self.pane_id.clone(),
        ]
    }

    fn release_arguments(&self) -> Vec<String> {
        vec![
            "pane".to_owned(),
            "release-agent".to_owned(),
            "--source".to_owned(),
            SOURCE.to_owned(),
            "--agent".to_owned(),
            AGENT_LABEL.to_owned(),
            self.pane_id.clone(),
        ]
    }
}

type InvokeFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

trait HerdrInvoker: Send + Sync + 'static {
    /// Invokes Herdr with arguments that exclude the executable itself.
    fn invoke(&self, arguments: Vec<String>) -> InvokeFuture<'_>;
}

struct ProcessHerdrInvoker {
    executable: String,
}

impl ProcessHerdrInvoker {
    fn new(executable: String) -> Self {
        Self { executable }
    }
}

impl HerdrInvoker for ProcessHerdrInvoker {
    fn invoke(&self, arguments: Vec<String>) -> InvokeFuture<'_> {
        Box::pin(async move {
            let mut child = Command::new(&self.executable)
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                // The report is optional, so abandoning it must not retain a child process.
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| format!("could not start Herdr reporter: {error}"))?;
            match timeout(REPORT_TIMEOUT, child.wait()).await {
                Ok(Ok(status)) if status.success() => Ok(()),
                Ok(Ok(status)) => Err(format!("Herdr reporter exited with {status}")),
                Ok(Err(error)) => Err(format!("could not wait for Herdr reporter: {error}")),
                Err(_) => {
                    let _ = child.start_kill();
                    Err("Herdr reporter timed out".to_owned())
                }
            }
        })
    }
}

enum ReporterMessage {
    Report(HerdrAgentState),
    Release(oneshot::Sender<()>),
}

/// Asynchronous, best-effort client runtime adapter for Herdr pane lifecycle reporting.
pub struct HerdrReporter {
    sender: mpsc::UnboundedSender<ReporterMessage>,
    _invoker: Arc<dyn HerdrInvoker>,
}

impl HerdrReporter {
    /// Creates a reporter only when Herdr supplied both required environment values.
    pub fn from_environment() -> Option<Self> {
        let configuration = HerdrConfiguration::from_environment(&ProcessEnvironment)?;
        Some(Self::new(
            configuration.clone(),
            ProcessHerdrInvoker::new(configuration.executable),
        ))
    }

    fn new<I: HerdrInvoker>(configuration: HerdrConfiguration, invoker: I) -> Self {
        let invoker = Arc::new(invoker);
        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(run_reporter(configuration, Arc::clone(&invoker), receiver));
        Self {
            sender,
            _invoker: invoker,
        }
    }

    /// Queues the semantic state for reporting without delaying the client or core.
    pub fn observe(&self, snapshot: &CoreSnapshot) {
        // The reporter owns the receiver; a closed channel only means shutdown has begun.
        let _ = self
            .sender
            .send(ReporterMessage::Report(herdr_state(snapshot)));
    }

    /// Releases pane lifecycle authority after normal client shutdown.
    pub async fn shutdown(&self) {
        self.shutdown_with_timeout(REPORT_TIMEOUT).await;
    }

    async fn shutdown_with_timeout(&self, deadline: Duration) {
        let (finished, receiver) = oneshot::channel();
        if self.sender.send(ReporterMessage::Release(finished)).is_ok() {
            // The actor may already be inside a report call, so one outer bound prevents that
            // call plus release from extending optional shutdown by two separate deadlines.
            let _ = timeout(deadline, receiver).await;
        }
    }
}

async fn run_reporter<I: HerdrInvoker>(
    configuration: HerdrConfiguration,
    invoker: Arc<I>,
    mut receiver: mpsc::UnboundedReceiver<ReporterMessage>,
) {
    let mut last_reported = None;
    while let Some(message) = receiver.recv().await {
        let message = coalesce(message, &mut receiver);
        match message {
            ReporterMessage::Report(state) if last_reported == Some(state) => {}
            ReporterMessage::Report(state) => {
                if invoker
                    .invoke(configuration.report_arguments(state))
                    .await
                    .is_ok()
                {
                    last_reported = Some(state);
                }
            }
            ReporterMessage::Release(finished) => {
                let _ = invoker.invoke(configuration.release_arguments()).await;
                let _ = finished.send(());
                return;
            }
        }
    }
}

fn coalesce(
    mut message: ReporterMessage,
    receiver: &mut mpsc::UnboundedReceiver<ReporterMessage>,
) -> ReporterMessage {
    while let Ok(next) = receiver.try_recv() {
        match next {
            ReporterMessage::Report(_) => message = next,
            ReporterMessage::Release(finished) => return ReporterMessage::Release(finished),
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex};

    #[derive(Default)]
    struct FakeEnvironment(BTreeMap<String, String>);

    impl HerdrEnvironment for FakeEnvironment {
        fn value(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    struct FakeInvoker {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        failures_remaining: Arc<Mutex<usize>>,
        called: Arc<tokio::sync::Notify>,
    }

    struct PendingInvoker {
        started: Arc<tokio::sync::Notify>,
    }

    impl HerdrInvoker for PendingInvoker {
        fn invoke(&self, _arguments: Vec<String>) -> InvokeFuture<'_> {
            Box::pin(async move {
                self.started.notify_one();
                std::future::pending::<Result<(), String>>().await
            })
        }
    }

    impl HerdrInvoker for FakeInvoker {
        fn invoke(&self, arguments: Vec<String>) -> InvokeFuture<'_> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("test call mutex must not be poisoned")
                    .push(arguments);
                self.called.notify_one();
                let mut failures = self
                    .failures_remaining
                    .lock()
                    .expect("test failure mutex must not be poisoned");
                if *failures == 0 {
                    Ok(())
                } else {
                    *failures -= 1;
                    Err("simulated failure".to_owned())
                }
            })
        }
    }

    fn configuration() -> HerdrConfiguration {
        HerdrConfiguration {
            executable: "fake-herdr".to_owned(),
            pane_id: "pane-7".to_owned(),
        }
    }

    #[test]
    fn environment_gates_reporting_and_selects_the_executable() {
        assert!(HerdrConfiguration::from_environment(&FakeEnvironment::default()).is_none());
        let invalid = FakeEnvironment(BTreeMap::from([
            ("HERDR_ENV".to_owned(), "unexpected".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "pane-7".to_owned()),
        ]));
        assert!(HerdrConfiguration::from_environment(&invalid).is_none());
        let environment = FakeEnvironment(BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "pane-7".to_owned()),
            ("HERDR_BIN_PATH".to_owned(), "/tmp/herdr".to_owned()),
        ]));
        assert_eq!(
            HerdrConfiguration::from_environment(&environment),
            Some(HerdrConfiguration {
                executable: "/tmp/herdr".to_owned(),
                pane_id: "pane-7".to_owned(),
            })
        );
        let fallback = FakeEnvironment(BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "pane-7".to_owned()),
            ("HERDR_BIN_PATH".to_owned(), " ".to_owned()),
        ]));
        assert_eq!(
            HerdrConfiguration::from_environment(&fallback)
                .expect("valid Herdr environment")
                .executable,
            "herdr"
        );
    }

    #[test]
    fn arguments_are_shell_free_and_exact() {
        let configuration = configuration();
        assert_eq!(
            configuration.report_arguments(HerdrAgentState::Blocked),
            [
                "pane",
                "report-agent",
                "--source",
                "misy:cli",
                "--agent",
                "misy",
                "--state",
                "blocked",
                "pane-7"
            ]
        );
        assert_eq!(
            configuration.release_arguments(),
            [
                "pane",
                "release-agent",
                "--source",
                "misy:cli",
                "--agent",
                "misy",
                "pane-7"
            ]
        );
    }

    #[test]
    fn question_state_has_priority_over_work() {
        assert_eq!(state_from_flags(false, false), HerdrAgentState::Idle);
        assert_eq!(state_from_flags(false, true), HerdrAgentState::Working);
        assert_eq!(state_from_flags(true, true), HerdrAgentState::Blocked);
    }

    #[tokio::test]
    async fn retries_failed_reports_and_deduplicates_successful_state() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::new(tokio::sync::Notify::new());
        let invoker = FakeInvoker {
            calls: Arc::clone(&calls),
            failures_remaining: Arc::new(Mutex::new(1)),
            called: Arc::clone(&called),
        };
        let reporter = HerdrReporter::new(configuration(), invoker);
        reporter
            .sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("reporter must be alive");
        called.notified().await;
        reporter
            .sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("reporter must be alive");
        called.notified().await;
        reporter
            .sender
            .send(ReporterMessage::Report(HerdrAgentState::Idle))
            .expect("reporter must be alive");
        reporter
            .sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("reporter must be alive");
        reporter.shutdown().await;
        let calls = calls
            .lock()
            .expect("test call mutex must not be poisoned")
            .clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0][7], "working");
        assert_eq!(calls[1][7], "working");
        assert_eq!(calls[2][1], "release-agent");
    }

    #[test]
    fn coalescing_keeps_only_the_latest_queued_state_and_prioritizes_release() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("receiver must be alive");
        sender
            .send(ReporterMessage::Report(HerdrAgentState::Idle))
            .expect("receiver must be alive");
        let first = receiver.try_recv().expect("first queued report");
        assert!(matches!(
            coalesce(first, &mut receiver),
            ReporterMessage::Report(HerdrAgentState::Idle)
        ));

        sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("receiver must be alive");
        let (finished, _waiter) = oneshot::channel();
        sender
            .send(ReporterMessage::Release(finished))
            .expect("receiver must be alive");
        let first = receiver.try_recv().expect("first queued report");
        assert!(matches!(
            coalesce(first, &mut receiver),
            ReporterMessage::Release(_)
        ));
    }

    #[tokio::test]
    async fn shutdown_has_one_outer_deadline_when_a_report_is_pending() {
        let started = Arc::new(tokio::sync::Notify::new());
        let reporter = HerdrReporter::new(
            configuration(),
            PendingInvoker {
                started: Arc::clone(&started),
            },
        );
        reporter
            .sender
            .send(ReporterMessage::Report(HerdrAgentState::Working))
            .expect("reporter must be alive");
        started.notified().await;

        let shutdown = reporter.shutdown_with_timeout(Duration::from_millis(10));
        assert!(
            timeout(Duration::from_millis(100), shutdown).await.is_ok(),
            "shutdown must not wait for a pending report and a later release separately"
        );
    }
}
