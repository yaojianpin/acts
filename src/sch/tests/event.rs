use crate::{
    sch::{event::EventHub, Event},
    Message, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn event_start() {
    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = EventHub::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnStart(Arc::new(move |w: &Workflow| {
        assert!(w.id == workflow2.id);
    })));

    evt.disp_start(&workflow);
}

#[tokio::test]
async fn event_finished() {
    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let evt = EventHub::new();
    let workflow2 = workflow.clone();
    evt.add_event(&Event::OnComplete(Arc::new(move |w: &Workflow| {
        assert!(w.id == workflow2.id);
    })));

    evt.disp_complete(&workflow);
}

#[tokio::test]
async fn event_error() {
    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let workflow_id = workflow.id.clone();

    let evt = EventHub::new();
    evt.add_event(&Event::OnError(Arc::new(move |w: &Workflow| {
        assert!(w.id == workflow_id);
    })));

    evt.disp_error(&workflow);
}

#[tokio::test]
async fn event_message() {
    let evt = EventHub::new();
    evt.add_event(&Event::OnMessage(Arc::new(move |message: &Message| {
        assert!(message.id == "m1");
    })));
    let m = Message::new("w1", "1", "u1", None);
    evt.disp_message(&m);
}
