use crate::{
    event::{Emitter, Event, Message},
    sch::{ActKind, Proc, Scheduler},
    utils, Vars, Workflow, WorkflowState,
};
use std::sync::Arc;

use super::{EventAction, MessageKind};

#[tokio::test]
async fn event_start() {
    let text = include_str!("../../examples/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();

    let (proc, _) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnStart(Arc::new(move |state: &WorkflowState| {
        assert!(state.mid == workflow2.id);
    })));

    let state = proc.workflow_state(&EventAction::Create);
    evt.dispatch_start_event(&state);
}

#[tokio::test]
async fn event_finished() {
    let text = include_str!("../../examples/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    let (proc, _) = create_proc(&mut workflow, &utils::longid());
    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnComplete(Arc::new(move |w: &WorkflowState| {
        assert!(w.mid == workflow2.id);
    })));

    let state = proc.workflow_state(&EventAction::Complete);
    evt.dispatch_complete_event(&state);
}

#[tokio::test]
async fn event_error() {
    let text = include_str!("../../examples/simple.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    let workflow_id = workflow.id.clone();

    let (proc, _) = create_proc(&mut workflow, &utils::longid());

    let evt = Emitter::new();
    evt.add_event(&Event::OnError(Arc::new(move |w: &WorkflowState| {
        assert!(w.mid == workflow_id);
    })));

    let state = proc.workflow_state(&EventAction::Error);
    evt.dispatch_error(&state);
}

#[tokio::test]
async fn event_message() {
    let evt = Emitter::new();
    evt.add_event(&Event::OnMessage(Arc::new(move |message: &Message| {
        assert!(message.pid == "w1");
    })));
    let m = Message {
        kind: MessageKind::Act(ActKind::User),
        event: EventAction::Create,
        mid: "mid".to_string(),
        topic: "m1".to_string(),
        nkind: "workflow".to_string(),
        nid: "a".to_string(),
        pid: "w1".to_string(),
        tid: "t1".to_string(),
        key: None,
        vars: Vars::new(),
    };

    evt.dispatch_message(&m);
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Scheduler>) {
    let scher = Scheduler::new();
    let proc = scher.create_proc(id, workflow);
    (proc, scher)
}
