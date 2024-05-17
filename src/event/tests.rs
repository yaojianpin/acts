use super::EventAction;
use crate::{
    event::{Emitter, MessageState},
    sch::{Proc, Runtime, TaskState},
    utils, Engine, Workflow,
};
use std::sync::Arc;

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
    let state: MessageState = "none".into();
    assert_eq!(state, MessageState::None);

    let state: MessageState = "error".into();
    assert_eq!(state, MessageState::Error);

    let state: MessageState = "aborted".into();
    assert_eq!(state, MessageState::Aborted);

    let state: MessageState = "submitted".into();
    assert_eq!(state, MessageState::Submitted);

    let state: MessageState = "cancelled".into();
    assert_eq!(state, MessageState::Cancelled);

    let state: MessageState = "backed".into();
    assert_eq!(state, MessageState::Backed);

    let state: MessageState = "created".into();
    assert_eq!(state, MessageState::Created);

    let state: MessageState = "skipped".into();
    assert_eq!(state, MessageState::Skipped);

    let state: MessageState = "removed".into();
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
    assert_eq!(action.is_err(), true);
}

#[tokio::test]
async fn event_on_proc() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (proc, rt) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_proc(move |e| {
        assert_eq!(e.inner().state(), TaskState::Running);
        assert_eq!(e.inner().model().id, workflow2.id);
    });
    proc.set_state(TaskState::Running);
    rt.scher().emit_proc_event(&proc);
}

#[tokio::test]
async fn event_on_task() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (proc, rt) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    evt.on_task(move |e| {
        assert_eq!(e.inner().state(), TaskState::Running);
    });
    proc.set_state(TaskState::Running);
    let task = proc.create_task(&proc.tree().root.as_ref().unwrap(), None);
    task.set_state(TaskState::Running);
    rt.scher().emit_task_event(&task).unwrap();
}

#[tokio::test]
async fn event_start() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (proc, _rt) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_start("k1", move |e| {
        assert!(e.model.id == workflow2.id);
    });
    proc.start();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_start_event(&message);
    }
}

#[tokio::test]
async fn event_finished() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (proc, _rt) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_complete("k1", move |e| {
        assert!(e.model.id == workflow2.id);
    });

    proc.start();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_complete_event(&message);
    }
}

#[tokio::test]
async fn event_error() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (proc, _rt) = create_proc(&mut workflow, &utils::longid());

    let evt = Emitter::new();
    evt.on_error("k1", move |e| {
        assert!(e.model.id == workflow_id);
    });

    proc.start();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_error(&message);
    }
}

#[tokio::test]
async fn event_message_default() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (proc, engine) = create_proc2(&mut workflow, &utils::longid());

    let (s1, s2) = engine.signal(false).double();
    let evt = Emitter::new();
    evt.on_message("k1", move |e| {
        s1.send(e.model.id == workflow_id);
    });

    proc.start();
    if let Some(root) = proc.root() {
        let message = root.create_message();
        evt.emit_message(&message);
    }
    let ret = s2.recv().await;
    assert!(ret);
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Runtime>) {
    let engine = Engine::new();
    let rt = engine.runtime();
    let proc = rt.create_proc(id, workflow);
    (proc, rt)
}

fn create_proc2(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Engine) {
    let engine = Engine::new();
    let rt = engine.runtime();
    let proc = rt.create_proc(id, workflow);
    (proc, engine)
}
