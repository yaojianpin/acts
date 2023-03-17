use crate::{
    emitter,
    sch::{event::EventData, ActionOptions, Proc, Scheduler},
    Emitter, Message, State, TaskState, UserMessage, Vars, Workflow,
};
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn task_state() {
    let mut workflow = create_simple_workflow();
    let (task, _, _) = create_proc(&mut workflow, "w1");
    assert!(task.state() == TaskState::None);
}

#[tokio::test]
async fn task_start() {
    let mut workflow = create_simple_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    proc.start();
    emitter.on_proc(|proc: &Proc, _data: &EventData| {
        assert_eq!(proc.state(), TaskState::Running);
    });
}

#[tokio::test]
async fn task_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = create_complete_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    emitter.on_message(move |msg: &Message| {
        let uid = &msg.uid.clone().unwrap();
        let message = UserMessage::new(&msg.pid, uid, "complete", None);
        s.sched_message(&message);
    });

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_complete(move |_| {
        *r.lock().unwrap() = true;
        s.close();
    });
    proc.start();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn task_cancel() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_cancel_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        let mut count = count.lock().unwrap();
        if msg.uid == Some("a".to_string()) && *count == 0 {
            let uid = &msg.uid.clone().unwrap();
            let message = UserMessage::new(&msg.pid, uid, "complete", None);
            s.sched_message(&message);
        } else if msg.uid == Some("b".to_string()) {
            // cancel the b's task by a
            let message = UserMessage::new(&msg.pid, "a", "cancel", None);
            s.sched_message(&message);
        } else if *count == 2 {
            *r.lock().unwrap() = msg.uid == Some("a".to_string());
            s.close();
        }

        *count += 1;
    });

    proc.start();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn task_back() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_back_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        let mut count = count.lock().unwrap();
        if msg.uid == Some("a".to_string()) && *count == 0 {
            let uid = &msg.uid.clone().unwrap();
            let message = UserMessage::new(&msg.pid, uid, "complete", None);
            s.sched_message(&message);
        } else if msg.uid == Some("b".to_string()) {
            let message = UserMessage::new(
                &msg.pid,
                "b",
                "back",
                Some(ActionOptions {
                    to: Some("step1".to_string()),
                    vars: Vars::new(),
                }),
            );
            s.sched_message(&message);
        } else if *count == 2 {
            *r.lock().unwrap() = msg.uid == Some("a".to_string());
            s.close();
        }

        *count += 1;
    });

    proc.start();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn task_abort() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_back_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        let mut count = count.lock().unwrap();
        if msg.uid == Some("a".to_string()) && *count == 0 {
            let uid = &msg.uid.clone().unwrap();
            let message = UserMessage::new(&msg.pid, uid, "abort", None);
            s.sched_message(&message);
        }
        *count += 1;
    });

    let s2 = scher.clone();
    emitter.on_complete(move |w: &State<Workflow>| {
        *r.lock().unwrap() = w.state.is_abort();
        s2.close();
    });
    proc.start();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn task_submit() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_submit_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        let mut count = count.lock().unwrap();
        if msg.uid == Some("a".to_string()) && *count == 0 {
            let uid = &msg.uid.clone().unwrap();
            let message = UserMessage::new(&msg.pid, uid, "submit", None);
            s.sched_message(&message);
        }
        *count += 1;
    });

    let s2 = scher.clone();
    emitter.on_complete(move |w: &State<Workflow>| {
        *r.lock().unwrap() = w.state.is_success();
        s2.close();
    });
    proc.start();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Proc, Arc<Scheduler>, Arc<Emitter>) {
    let scher = Arc::new(Scheduler::new());

    workflow.set_biz_id(id);
    let proc = scher.create_raw_proc(workflow);

    let emitter = Arc::new(Emitter::new(&scher));

    (proc.clone(), scher, emitter)
}

fn create_simple_workflow() -> Workflow {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}

fn create_complete_workflow() -> Workflow {
    let text = include_str!("./models/complete.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}

fn create_cancel_workflow() -> Workflow {
    let text = include_str!("./models/cancel.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}

fn create_back_workflow() -> Workflow {
    let text = include_str!("./models/back.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}

fn create_submit_workflow() -> Workflow {
    let text = include_str!("./models/submit.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
