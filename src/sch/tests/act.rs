use serde_json::json;

use crate::{
    event::{Action, Emitter, EventAction, Message, MessageKind},
    sch::{ActKind, Proc, Scheduler},
    utils, Engine, Executor, Manager, Vars, Workflow, WorkflowState,
};
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn sch_act_scr_send() {
    let mut workflow = create_send_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let s = scher.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_notice_message() {
            assert_eq!(msg.key, "123");
            s.close();
        }
    });

    proc.start(&scher);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_act_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = create_complete_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let uid = msg.uid;

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&msg.pid, &msg.aid, "complete", &options);
            s.do_action(&action).unwrap();
        }
        match &msg.kind {
            MessageKind::Act(act_kind) => if act_kind == &ActKind::User {},
            _ => {}
        };
    });

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_complete(move |_| {
        *r.lock().unwrap() = true;
        s.close();
    });
    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_normal() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_cancel_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();

    let act_id = Arc::new(Mutex::new(None));
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let mut count = count.lock().unwrap();
            if msg.uid == "a" && msg.event == &EventAction::Create {
                if *count == 0 {
                    *act_id.lock().unwrap() = Some(msg.aid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(msg.uid.to_string()));

                    let action = Action::new(&msg.pid, &msg.aid, "complete", &options);
                    s.do_action(&action).unwrap();
                } else {
                    *r.lock().unwrap() = true;
                    s.close();
                }
            } else if msg.uid == "b" && msg.event == &EventAction::Create {
                // cancel the b's task by a
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!("a".to_string()));

                // get the completed act id in previous step
                let act_id = &*act_id.lock().unwrap();
                let aid = act_id.as_deref().unwrap();
                let action = Action::new(&msg.pid, aid, "cancel", &options);
                s.do_action(&action).unwrap();
            }
            *count += 1;
        }
    });

    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_back() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_back_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let mut count = count.lock().unwrap();
            if msg.uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(msg.uid.to_string()));

                let action = Action::new(&msg.pid, &msg.aid, "complete", &options);
                s.do_action(&action).unwrap();
            } else if msg.uid == "b" {
                if msg.event == &EventAction::Create {
                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!("b".to_string()));
                    options.insert("to".to_string(), json!("step1".to_string()));
                    let action = Action::new(msg.pid, &msg.aid, "back", &options);
                    s.do_action(&action).unwrap();
                }
            } else if msg.event == &EventAction::Create && msg.uid == "a" && *count > 0 {
                *r.lock().unwrap() = msg.uid == "a";
                s.close();
            }

            *count += 1;
        }
    });

    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_abort() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_back_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let mut count = count.lock().unwrap();
            if msg.uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(msg.uid.to_string()));

                let message = Action::new(&msg.pid, &msg.aid, "abort", &options);
                s.do_action(&message).unwrap();
            }
            *count += 1;
        }
    });

    let s2 = scher.clone();
    emitter.on_complete(move |w: &WorkflowState| {
        *r.lock().unwrap() = w.state.is_abort();
        s2.close();
    });
    proc.start(&scher);
    scher.event_loop().await;
    let result = *ret.lock().unwrap();
    assert!(result);
}

#[tokio::test]
async fn sch_act_submit() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = create_submit_workflow();
    workflow.id = utils::longid();

    let pid = utils::longid();
    let (_, scher, emitter, executor, manager) = create_proc2(&mut workflow, &pid);
    manager.deploy(&workflow).unwrap();

    let s = scher.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            if msg.uid == "a" && msg.event == &EventAction::Create {
                let mut options = Vars::new();
                options.insert("uid".to_string(), msg.uid.into());

                let action = Action::new(msg.pid, &msg.aid, "complete", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    let s2 = scher.clone();
    let r = ret.clone();
    emitter.on_complete(move |w: &WorkflowState| {
        *r.lock().unwrap() = w.state.is_success();
        s2.close();
    });

    let mut options = Vars::new();
    options.insert("uid".to_string(), "Tom".into());
    executor.submit(&workflow.id, &options).unwrap();
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_not_support_action() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = create_submit_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let mut count = count.lock().unwrap();
            if msg.uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(msg.uid.to_string()));

                let action = Action::new(msg.pid, &msg.aid, "not_support", &options);
                *r.lock().unwrap() = s.do_action(&action).is_err();
                s.close();
            }
            *count += 1;
        }
    });

    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_next_by_complete_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = create_complete_workflow();
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let uid = msg.uid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&msg.pid, &msg.aid, "complete", &options);
            s.do_action(&action).unwrap();

            // action again
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
        match &msg.kind {
            MessageKind::Act(act_kind) => if act_kind == &ActKind::User {},
            _ => {}
        };
    });

    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_by_running_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = create_complete_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg: &Message| {
        if let Some(msg) = msg.as_user_message() {
            let uid = msg.uid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&msg.pid, &msg.aid, "cancel", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    proc.start(&scher);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

// #[tokio::test]
// async fn sch_act_load_from_store() {
//     let mut workflow = create_complete_workflow();
//     let id = utils::longid();
//     let tr = NodeTree::build(&mut workflow);
//     let (proc, scher, emitter) = create_proc(&mut workflow, &id);
//     let task = proc.create_task(tr.root.as_ref().unwrap(), None);
//     let act = Act::new(&task, ActKind::Action, &Vars::new());
//     proc.push_act(&act);
//     assert!(scher.act(&act.id).is_some())
// }

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = scher.create_raw_proc(pid, workflow);

    let emitter = scher.emitter();

    (Arc::new(proc), scher, emitter)
}

fn create_proc2(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Proc>,
    Arc<Scheduler>,
    Arc<Emitter>,
    Arc<Executor>,
    Arc<Manager>,
) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = scher.create_raw_proc(pid, workflow);

    let emitter = scher.emitter();
    let executor = engine.executor();
    let manager = engine.manager();
    (Arc::new(proc), scher, emitter, executor, manager)
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

fn create_send_workflow() -> Workflow {
    let text = include_str!("./models/send.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
