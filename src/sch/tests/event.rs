use crate::{
    sch::{event::EventHub, Event, TaskState},
    Message, ModelBase, State, Vars, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn event_start() {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = EventHub::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnStart(Arc::new(move |state: &State<Workflow>| {
        assert!(state.id() == workflow2.id);
    })));

    let state = State {
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.on_start(&state);
}

#[tokio::test]
async fn event_finished() {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = EventHub::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnComplete(Arc::new(move |w: &State<Workflow>| {
        assert!(w.id() == workflow2.id);
    })));

    let state = State {
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.on_complete(&state);
}

#[tokio::test]
async fn event_error() {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let workflow_id = workflow.id.clone();

    let evt = EventHub::new();
    evt.add_event(&Event::OnError(Arc::new(move |w: &State<Workflow>| {
        assert!(w.id() == workflow_id);
    })));

    let state = State {
        node: Arc::new(workflow),
        state: TaskState::None,
        start_time: 0,
        end_time: 0,
        outputs: Vars::new(),
    };
    evt.on_error(&state);
}

#[tokio::test]
async fn event_message() {
    let evt = EventHub::new();
    evt.add_event(&Event::OnMessage(Arc::new(move |message: &Message| {
        assert!(message.id == "m1");
    })));
    let m = Message::new("w1", "1", Some("u1".to_string()));
    evt.on_message(&m);
}
