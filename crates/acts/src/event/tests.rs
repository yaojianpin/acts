use super::EventAction;
use crate::{
    Workflow,
    event::{Emitter, MessageState},
    scheduler::TaskState,
    utils::{self, test::create_proc},
};
use std::str::FromStr;

#[test]
fn event_message_state_to_string() {
    let state = MessageState::None;
    assert_eq!(state.to_string(), "none");

    let state = MessageState::Created;
    assert_eq!(state.to_string(), "created");

    let state = MessageState::Error;
    assert_eq!(state.to_string(), "error");

    let state = MessageState::Submitted;
    assert_eq!(state.to_string(), "submitted");

    let state = MessageState::Cancelled;
    assert_eq!(state.to_string(), "cancelled");

    let state = MessageState::Backed;
    assert_eq!(state.to_string(), "backed");

    let state = MessageState::Aborted;
    assert_eq!(state.to_string(), "aborted");

    let state = MessageState::Removed;
    assert_eq!(state.to_string(), "removed");

    let state = MessageState::Skipped;
    assert_eq!(state.to_string(), "skipped");
}

#[test]
fn event_message_state_from_string() {
    let state = MessageState::from_str("none").unwrap();
    assert_eq!(state, MessageState::None);

    let state = MessageState::from_str("error").unwrap();
    assert_eq!(state, MessageState::Error);

    let state = MessageState::from_str("aborted").unwrap();
    assert_eq!(state, MessageState::Aborted);

    let state = MessageState::from_str("submitted").unwrap();
    assert_eq!(state, MessageState::Submitted);

    let state = MessageState::from_str("cancelled").unwrap();
    assert_eq!(state, MessageState::Cancelled);

    let state = MessageState::from_str("backed").unwrap();
    assert_eq!(state, MessageState::Backed);

    let state = MessageState::from_str("created").unwrap();
    assert_eq!(state, MessageState::Created);

    let state = MessageState::from_str("skipped").unwrap();
    assert_eq!(state, MessageState::Skipped);

    let state = MessageState::from_str("removed").unwrap();
    assert_eq!(state, MessageState::Removed);
}

#[test]
fn event_message_state_from_task_state() {
    let state: MessageState = TaskState::None.into();
    assert_eq!(state, MessageState::None);

    let state: MessageState = TaskState::Error.into();
    assert_eq!(state, MessageState::Error);

    let state: MessageState = TaskState::Aborted.into();
    assert_eq!(state, MessageState::Aborted);

    let state: MessageState = TaskState::Submitted.into();
    assert_eq!(state, MessageState::Submitted);

    let state: MessageState = TaskState::Cancelled.into();
    assert_eq!(state, MessageState::Cancelled);

    let state: MessageState = TaskState::Backed.into();
    assert_eq!(state, MessageState::Backed);

    let state: MessageState = TaskState::Running.into();
    assert_eq!(state, MessageState::Created);

    let state: MessageState = TaskState::Pending.into();
    assert_eq!(state, MessageState::Created);

    let state: MessageState = TaskState::Interrupt.into();
    assert_eq!(state, MessageState::Created);

    let state: MessageState = TaskState::Skipped.into();
    assert_eq!(state, MessageState::Skipped);

    let state: MessageState = TaskState::Removed.into();
    assert_eq!(state, MessageState::Removed);
}

#[tokio::test]
async fn event_action_parse() {
    let action = EventAction::parse("next").unwrap();
    assert_eq!(action, EventAction::Next);

    let action = EventAction::parse("submit").unwrap();
    assert_eq!(action, EventAction::Submit);

    let action = EventAction::parse("cancel").unwrap();
    assert_eq!(action, EventAction::Cancel);

    let action = EventAction::parse("back").unwrap();
    assert_eq!(action, EventAction::Back);

    let action = EventAction::parse("abort").unwrap();
    assert_eq!(action, EventAction::Abort);

    let action = EventAction::parse("skip").unwrap();
    assert_eq!(action, EventAction::Skip);

    let action = EventAction::parse("error").unwrap();
    assert_eq!(action, EventAction::Error);

    let action = EventAction::parse("aaaaa");
    assert!(action.is_err());
}

#[tokio::test]
async fn event_on_proc() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_proc(move |e| {
        let workflow2 = workflow2.clone();
        async move {
            assert_eq!(e.inner().state(), TaskState::Running);
            assert_eq!(e.inner().model().id, workflow2.id);
        }
    });
    proc.set_state(TaskState::Running);
    engine.runtime().emitter().emit_proc_event(&proc).await;
}

#[tokio::test]
async fn event_on_task() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();
    evt.on_task(move |e| async move {
        assert_eq!(e.inner().state(), TaskState::Running);
    });
    proc.set_state(TaskState::Running);
    let task = proc
        .create_task(proc.tree().root.as_ref().unwrap(), None)
        .unwrap();
    task.set_state(TaskState::Running);
    engine
        .runtime()
        .emitter()
        .emit_task_event(&task)
        .await
        .unwrap();
}

#[tokio::test]
async fn event_start() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (_, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_start("k1", move |e| {
        let workflow2 = workflow2.clone();
        async move {
            assert!(e.mid == workflow2.id);
        }
    });
    proc.start().await.unwrap();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_start_event(&message);
    }
}

#[tokio::test]
async fn event_finished() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (_, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_complete("k1", move |e| {
        let workflow2 = workflow2.clone();
        async move {
            assert!(e.mid == workflow2.id);
        }
    });

    proc.start().await.unwrap();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_complete_event(&message);
    }
}

#[tokio::test]
async fn event_error() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (_, proc) = create_proc(&workflow, &utils::longid()).await;

    let evt = Emitter::new();
    evt.on_error("k1", move |e| {
        let workflow_id = workflow_id.clone();
        async move {
            assert!(e.mid == workflow_id);
        }
    });

    proc.start().await.unwrap();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_error(&message);
    }
}

#[tokio::test]
async fn event_message_default() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;

    let (s1, s2) = engine.signal(false).double();
    let evt = Emitter::new();
    evt.on_message("k1", move |e| {
        let s1 = s1.clone();
        let workflow_id = workflow_id.clone();
        async move {
            s1.send(e.mid == workflow_id);
        }
    });

    proc.start().await.unwrap();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_message(&message);
    }
    let ret = s2.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn event_message_dup_key() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;

    let (s1, s2) = engine.signal(false).double();
    let evt = Emitter::new();
    evt.on_message("k1", move |_| async {});
    evt.on_message("k1", move |e| {
        let s1 = s1.clone();
        let workflow_id = workflow_id.clone();
        async move {
            s1.send(e.mid == workflow_id);
        }
    });

    proc.start().await.unwrap();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_message(&message);
    }
    let ret = s2.recv().await;
    assert!(ret);
}

#[tokio::test(flavor = "multi_thread")]
async fn event_message_fifo_order() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;

    let (s1, s2) = engine.signal(Vec::new()).double();
    let evt = Emitter::new();
    evt.on_message("k1", move |e| {
        let s1 = s1.clone();
        async move {
            let is_last = e.mid == "mid-2";
            let mid = e.mid.clone();
            s1.update(move |data| data.push(mid.clone()));
            if is_last {
                s1.close();
            }
        }
    });

    proc.start().await.unwrap();
    for i in 0..3 {
        let msg = crate::Message {
            mid: format!("mid-{i}"),
            ..Default::default()
        };
        evt.emit_message(&msg);
    }

    let ret = s2.recv().await;
    assert_eq!(ret, vec!["mid-0", "mid-1", "mid-2"]);
}

/// A worker that exits on its terminal event must not drop an event routed to
/// it while the terminal handler was still running: the exit path re-homes the
/// queued event to a fresh worker instead of dropping it with the receiver.
#[tokio::test]
async fn event_terminal_exit_rehomes_queued_event() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();

    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    {
        let entered = entered.clone();
        let release = release.clone();
        evt.on_error("k1", move |_| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
            }
        });
    }
    let (s1, s2) = engine.signal(false).double();
    evt.on_message("k1", move |_| {
        let s1 = s1.clone();
        async move {
            s1.send(true);
        }
    });

    proc.start().await.unwrap();
    let root = proc.root().unwrap();
    evt.emit_error(&root.create_message());
    // the terminal handler is now running; queue a follow-up for the same pid
    entered.notified().await;
    evt.emit_message(&root.create_message());
    release.notify_one();

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), s2.recv())
        .await
        .expect("queued event was dropped when the terminal worker exited");
    assert!(received);
}

/// The report's exact scenario: a retry-timer delivery is routed to a process
/// whose terminal event is being handled. The delivery is queued behind the
/// terminal event and must still reach its channel handler when the worker
/// exits on that terminal event — otherwise the row waits out a whole retry
/// window, consuming another attempt of a budget that may already be spent.
///
/// Distinct from `event_terminal_exit_rehomes_queued_event`: a delivery takes
/// its own `KeyEvent` arm and is dispatched to the single handler of its
/// `chan_id` (not to every message handler), so the re-home path is pinned on
/// the delivery route specifically.
#[tokio::test]
async fn event_terminal_exit_rehomes_queued_delivery() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (_engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();

    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    {
        let entered = entered.clone();
        let release = release.clone();
        evt.on_error("k1", move |_| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                entered.notify_one();
                release.notified().await;
            }
        });
    }

    proc.start().await.unwrap();
    let root = proc.root().unwrap();

    // the channel handler the retry timer re-sends to; registered after start
    // so only the delivery can reach it
    let delivered = std::sync::Arc::new(tokio::sync::Notify::new());
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    {
        let delivered = delivered.clone();
        let calls = calls.clone();
        evt.on_message("client-1", move |_| {
            let delivered = delivered.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                delivered.notify_one();
            }
        });
    }

    // terminal event first: its handler holds the worker inside the loop
    evt.emit_error(&root.create_message());
    entered.notified().await;
    // the retry timer re-sends a delivery of the same process; it is queued
    // behind the terminal event and the worker breaks right after it
    evt.emit_delivery("client-1", &root.create_message());
    release.notify_one();

    tokio::time::timeout(std::time::Duration::from_secs(5), delivered.notified())
        .await
        .expect("queued delivery was dropped when the terminal worker exited");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the delivery reached its channel handler exactly once"
    );
}

/// A routing entry whose worker ended without running its re-home exit (its
/// task was dropped, e.g. the runtime aborted it) must not swallow events: the
/// next event replaces the stale sender with a fresh worker instead of being
/// written into a queue nobody reads.
///
/// The event used to create the worker has no registered handler, so only the
/// event routed after the abandonment can reach the handler — the assertion
/// covers delivery, not the orphan's own dispatch.
#[tokio::test]
async fn event_route_replaces_an_abandoned_worker() {
    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let evt = Emitter::new();

    let (s1, s2) = engine.signal(Vec::new()).double();
    evt.on_message("k1", move |e| {
        let s1 = s1.clone();
        async move {
            let mid = e.mid.clone();
            s1.update(move |data| data.push(mid.clone()));
            s1.close();
        }
    });

    proc.start().await.unwrap();
    let root = proc.root().unwrap();
    // creates the process's worker; nothing is registered on the start key
    evt.emit_start_event(&root.create_message());
    // the worker task is gone without the exit path that would have replaced
    // its entry — the sender now outlives its receiver
    evt.abandon_worker(proc.id());

    let msg = crate::Message {
        mid: "mid-1".to_string(),
        ..root.create_message()
    };
    evt.emit_message(&msg);

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), s2.recv())
        .await
        .expect("the event was written into the abandoned worker's queue");
    assert_eq!(
        received,
        vec!["mid-1"],
        "the fresh worker delivered the event exactly once"
    );
}
