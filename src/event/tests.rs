use crate::{
    event::{Emitter, Event, Message},
    sch::TaskState,
    ModelBase, State, Vars, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn event_start() {
    let text = include_str!("../../examples/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnStart(Arc::new(move |state: &State<Workflow>| {
        assert!(state.id() == workflow2.id);
    })));

    let state = State {
        pid: "w1".to_string(),
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.dispatch_start_event(&state);
}

#[tokio::test]
async fn event_finished() {
    let text = include_str!("../../examples/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = Emitter::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnComplete(Arc::new(move |w: &State<Workflow>| {
        assert!(w.id() == workflow2.id);
    })));

    let state = State {
        pid: "w1".to_string(),
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.dispatch_complete_event(&state);
}

#[tokio::test]
async fn event_error() {
    let text = include_str!("../../examples/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let workflow_id = workflow.id.clone();

    let evt = Emitter::new();
    evt.add_event(&Event::OnError(Arc::new(move |w: &State<Workflow>| {
        assert!(w.id() == workflow_id);
    })));

    let state = State {
        pid: "w1".to_string(),
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.dispatch_error(&state);
}

#[tokio::test]
async fn event_message() {
    let evt = Emitter::new();
    evt.add_event(&Event::OnMessage(Arc::new(move |message: &Message| {
        assert!(message.id == "m1");
    })));
    let m = Message::new("w1", "1", Some("u1".to_string()), Vars::new());
    evt.dispatch_message(&m);
}
