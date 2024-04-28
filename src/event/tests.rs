use super::{EventAction, MessageState};
use crate::{
    event::{Emitter, Message, Model},
    sch::{Proc, Scheduler, TaskState},
    utils, Engine, NodeKind, Vars, Workflow,
};
use std::sync::Arc;
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

    let (proc, scher) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_proc(move |e| {
        assert_eq!(e.inner().state(), TaskState::Running);
        assert_eq!(e.inner().model().id, workflow2.id);
    });
    proc.set_state(TaskState::Running);
    scher.emitter().emit_proc_event(&proc);
}

#[tokio::test]
async fn event_on_task() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (proc, scher) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    evt.on_task(move |e| {
        assert_eq!(e.inner().state(), TaskState::Running);
    });
    proc.set_state(TaskState::Running);
    let task = proc.create_task(&proc.tree().root.as_ref().unwrap(), None);
    task.set_state(TaskState::Running);
    scher.emitter().emit_task_event(&task).unwrap();
}

#[tokio::test]
async fn event_start() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));

    let (proc, _) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_start(move |e| {
        assert!(e.inner().mid == workflow2.id);
    });

    let state = proc.workflow_state();
    evt.emit_start_event(&state);
}

#[tokio::test]
async fn event_finished() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let (proc, _) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.on_complete(move |e| {
        assert!(e.inner().mid == workflow2.id);
    });

    let state = proc.workflow_state();
    evt.emit_complete_event(&state);
}

#[tokio::test]
async fn event_error() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let workflow_id = workflow.id.clone();
    let (proc, _) = create_proc(&mut workflow, &utils::longid());

    let evt = Emitter::new();
    evt.on_error(move |e| {
        assert!(e.inner().mid == workflow_id);
    });

    let state = proc.workflow_state();
    evt.emit_error(&state);
}

#[tokio::test]
async fn event_message() {
    let evt = Emitter::new();
    evt.on_message(move |e| {
        assert!(e.inner().proc_id == "w1");
    });
    let m = Message {
        id: "a1".to_string(),
        r#type: "msg".to_string(),
        source: NodeKind::Act.to_string(),
        state: MessageState::Created,
        model: Model {
            id: "mid".to_string(),
            name: "mname".to_string(),
            tag: "mtag".to_string(),
        },
        proc_id: "w1".to_string(),
        name: "a1".to_string(),
        inputs: Vars::new(),
        outputs: Vars::new(),
        tag: "".to_string(),
        start_time: 0,
        end_time: 0,
        key: "n1".to_string(),
    };

    evt.emit_message(&m);
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Scheduler>) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = scher.create_proc(id, workflow);
    (proc, scher)
}
