use super::{
    ActivityEvent, ActivityManager, ActivityRecord, ActivityState, MAX_ACTIVE_TASKS, StopReason,
    SupervisorTask, output::OrderedOutput, process,
};
use crate::{ActivityId, ActivityStatus, AgentId, ToolCall, activity::ActivityOwner};
use serde_json::json;
use std::{
    io,
    sync::{Arc, Barrier, Condvar, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, watch};

#[cfg(unix)]
use super::super::command::CommandRequest;

fn record(id: u64, status: ActivityStatus) -> Arc<ActivityRecord> {
    record_for_owner(id, status, ActivityOwner::Main)
}

fn record_for_owner(id: u64, status: ActivityStatus, owner: ActivityOwner) -> Arc<ActivityRecord> {
    let (stop, _) = watch::channel(None::<StopReason>);
    let mut output = OrderedOutput::default();
    output.append(
        crate::ActivityOutputStream::Stdout,
        format!("output-{id}").as_bytes(),
    );
    Arc::new(ActivityRecord {
        id: ActivityId::new(id),
        owner,
        title: format!("activity-{id}"),
        cwd: None,
        started_at_ms: id,
        interactive: false,
        permit: Mutex::new(None),
        input: Mutex::new(super::ActivityInput::Closed),
        state: Mutex::new(ActivityState {
            status,
            exit_code: status.is_terminal().then_some(0),
            output,
            message: None,
            published: false,
            terminal_event_sent: false,
        }),
        delivery_cursor: Mutex::new(super::output::DeliveryCursor::default()),
        changed: Notify::new(),
        stop,
        supervisor: Mutex::new(SupervisorTask::Unavailable),
        supervisor_changed: Notify::new(),
        interaction: tokio::sync::Mutex::new(()),
        poll_cancel: watch::channel(0).0,
        session_open: std::sync::atomic::AtomicBool::new(true),
    })
}

#[test]
fn command_admission_reserves_capacity_for_main_and_limits_each_child() {
    let manager = ActivityManager::new();
    let agents = [AgentId::new(1), AgentId::new(2), AgentId::new(3)];
    let child_permits = agents
        .into_iter()
        .flat_map(|agent| {
            (0..16).map({
                let manager = manager.clone();
                move |_| {
                    manager
                        .reserve_slot(ActivityOwner::Agent(agent))
                        .expect("reserve child process")
                }
            })
        })
        .collect::<Vec<_>>();
    assert!(
        manager
            .reserve_slot(ActivityOwner::Agent(AgentId::new(1)))
            .is_err()
    );
    let main_permits = (0..16)
        .map(|_| {
            manager
                .reserve_slot(ActivityOwner::Main)
                .expect("reserve main process")
        })
        .collect::<Vec<_>>();
    assert!(manager.reserve_slot(ActivityOwner::Main).is_err());
    drop(child_permits);
    drop(main_permits);
}

#[test]
fn model_task_roster_is_scoped_to_its_owner() {
    let manager = ActivityManager::new();
    let main = record_for_owner(1, ActivityStatus::Running, ActivityOwner::Main);
    let child_owner = ActivityOwner::Agent(AgentId::new(1));
    let child = record_for_owner(2, ActivityStatus::Running, child_owner);
    manager.publish(&main).expect("publish main command");
    manager.publish(&child).expect("publish child command");
    assert_eq!(manager.activities_for_owner(ActivityOwner::Main).len(), 1);
    assert_eq!(manager.activities_for_owner(child_owner).len(), 1);
    assert!(!manager.stop_for_owner(ActivityOwner::Main, child.id));
    assert!(manager.stop_for_owner(child_owner, child.id));
}

#[test]
fn finish_and_publish_race_emits_one_terminal_event() {
    for id in 1..=64 {
        let manager = ActivityManager::new();
        let mut events = manager.subscribe();
        let record = record(id, ActivityStatus::Running);
        manager
            .inner
            .records
            .lock()
            .expect("activity records mutex")
            .insert(record.id, Arc::clone(&record));
        let start = Arc::new(Barrier::new(3));

        std::thread::scope(|scope| {
            let publisher = manager.clone();
            let published_record = Arc::clone(&record);
            let publish_start = Arc::clone(&start);
            scope.spawn(move || {
                publish_start.wait();
                publisher
                    .publish(&published_record)
                    .expect("publish activity");
            });
            let weak_manager = Arc::downgrade(&manager.inner);
            let finished_record = Arc::clone(&record);
            let finish_start = Arc::clone(&start);
            scope.spawn(move || {
                finish_start.wait();
                process::finish_record(
                    &weak_manager,
                    &finished_record,
                    process::Completion::Failed("fixture failure".to_owned()),
                );
            });
            start.wait();
        });

        let mut terminal_count = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event, ActivityEvent::Finished(_)) {
                terminal_count += 1;
            }
        }
        assert_eq!(terminal_count, 1, "terminal event count for task-{id}");
    }
}

#[test]
fn terminal_event_burst_is_lossless_after_registry_eviction() {
    const EVENT_COUNT: u64 = 96;
    let manager = ActivityManager::new();
    let mut events = manager.subscribe();

    for id in 1..=EVENT_COUNT {
        manager
            .publish(&record(id, ActivityStatus::Completed))
            .expect("publish completed activity");
    }

    let mut outputs = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ActivityEvent::Finished(output) = event {
            outputs.push(output);
        }
    }
    assert_eq!(outputs.len(), EVENT_COUNT as usize);
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.activity.id.get())
            .collect::<Vec<_>>(),
        (1..=EVENT_COUNT).collect::<Vec<_>>()
    );
    assert!(
        outputs
            .iter()
            .all(|output| { output.stdout == format!("output-{}", output.activity.id.get()) })
    );
    assert_eq!(manager.activities().len(), 20);
}

#[cfg(unix)]
#[tokio::test]
async fn foreground_promotion_keeps_its_existing_process_permit() {
    let manager = ActivityManager::new();
    let request = CommandRequest::from_call(
        &ToolCall::new(
            "foreground-permit",
            "exec_command",
            json!({"cmd": "sleep 0.4; printf done"}),
        ),
        None,
    )
    .expect("parse foreground command");
    let pending = manager.start(&request).expect("start foreground command");
    let other_permits = (1..MAX_ACTIVE_TASKS)
        .map(|_| {
            manager
                .reserve_slot(ActivityOwner::Main)
                .expect("reserve remaining permit")
        })
        .collect::<Vec<_>>();

    let promoted = pending
        .wait_or_promote(Duration::from_millis(250), None, 10_000)
        .await;

    assert_eq!(promoted.activity.status, ActivityStatus::Running);
    assert!(manager.stop(promoted.activity.id));
    let stopped = manager
        .output(promoted.activity.id, Some(Duration::from_secs(2)))
        .await
        .expect("published foreground output");
    assert_eq!(stopped.activity.status, ActivityStatus::Stopped);
    drop(other_permits);
    manager
        .reserve_slot(ActivityOwner::Main)
        .expect("completed process releases permit");
}

#[cfg(unix)]
#[tokio::test]
async fn exhausted_process_limit_rejects_before_spawn() {
    let manager = ActivityManager::new();
    let permits = (0..MAX_ACTIVE_TASKS)
        .map(|_| {
            manager
                .reserve_slot(ActivityOwner::Main)
                .expect("reserve process permit")
        })
        .collect::<Vec<_>>();
    let request = CommandRequest::from_call(
        &ToolCall::new(
            "rejected-process",
            "exec_command",
            json!({"cmd": "printf should-not-run"}),
        ),
        None,
    )
    .expect("parse rejected command");

    let error = match manager.start(&request) {
        Ok(_) => panic!("process limit must reject command"),
        Err(error) => error,
    };

    assert!(error.contains("64 processes are already active"), "{error}");
    drop(permits);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_joins_timed_out_capture_before_final_output() {
    let manager = ActivityManager::new();
    let mut events = manager.subscribe();
    let record = record(1, ActivityStatus::Running);
    manager.publish(&record).expect("publish activity");
    assert!(matches!(
        events.try_recv().expect("activity changed event"),
        ActivityEvent::Changed(_)
    ));

    let append_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let capture_gate = Arc::clone(&append_gate);
    let capture_record = Arc::clone(&record);
    let (capture_started, started) = tokio::sync::oneshot::channel();
    let stdout = tokio::task::spawn_blocking(move || {
        let _ = capture_started.send(());
        let (released, wake) = &*capture_gate;
        let mut released = released.lock().expect("capture gate mutex");
        while !*released {
            released = wake.wait(released).expect("capture gate mutex");
        }
        capture_record
            .state
            .lock()
            .expect("activity state mutex")
            .output
            .append(crate::ActivityOutputStream::Stdout, b"-late");
        Ok::<(), io::Error>(())
    });
    started.await.expect("capture task started");
    let weak_manager = Arc::downgrade(&manager.inner);
    let finished_record = Arc::clone(&record);
    let supervisor = tokio::spawn(async move {
        process::drain_streams(
            process::SpawnedStreams {
                stdout: Some(stdout),
                stderr: None,
            },
            &finished_record,
            Duration::from_millis(10),
        )
        .await;
        process::finish_record(
            &weak_manager,
            &finished_record,
            process::Completion::Failed("fixture failure".to_owned()),
        );
    });
    record.set_supervisor(supervisor);

    let routed = Arc::new(Mutex::new(Vec::new()));
    let routed_events = Arc::clone(&routed);
    let router = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                ActivityEvent::Changed(_) => {}
                ActivityEvent::Finished(output) => routed_events
                    .lock()
                    .expect("routed events mutex")
                    .push(("finished", output.stdout, output.stdout_truncated)),
                ActivityEvent::Flush(completion) => {
                    routed_events.lock().expect("routed events mutex").push((
                        "shutdown",
                        String::new(),
                        false,
                    ));
                    let _ = completion.send(());
                    return;
                }
            }
        }
    });

    let shutting_down = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager.shutdown().await;
            manager.flush_events().await;
        }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        routed.lock().expect("routed events mutex").is_empty(),
        "capture cancellation must finish before terminal output is emitted"
    );
    let (released, wake) = &*append_gate;
    *released.lock().expect("capture gate mutex") = true;
    wake.notify_one();

    shutting_down.await.expect("activity shutdown");
    router.await.expect("activity router");
    assert_eq!(
        *routed.lock().expect("routed events mutex"),
        [
            ("finished", "output-1-late".to_owned(), true),
            ("shutdown", String::new(), false),
        ]
    );
}
