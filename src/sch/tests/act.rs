mod catch;
mod r#for;
mod r#use;

use crate::{
    event::{Action, ActionState, Emitter},
    sch::{Proc, Scheduler, TaskState},
    utils, Engine, Executor, Manager, Vars, Workflow,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn sch_act_create() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(|act| act.with_id("act1").with_name("act 1"))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_type("act") {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_needs_pending() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(|act| act.with_id("act1").with_need("act2"))
            .with_act(|act| act.with_id("act2"))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_type("act") {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Pending
    );
}

#[tokio::test]
async fn sch_act_needs_resume() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(|act| act.with_id("act1").with_need("act2"))
            .with_act(|act| act.with_id("act2"))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_key("act2") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            e.do_action(&e.proc_id, &e.id, "complete", &options)
                .unwrap();
        }

        if e.is_key("act1") {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("u1"))
        })
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let r = ret.clone();
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            if e.inner().state() == ActionState::Created {
                let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
                if let Err(err) = s.do_action(&action) {
                    println!("error: {}", err);
                    *r.lock().unwrap() = false;
                } else {
                    *r.lock().unwrap() = true;
                }
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;

    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_normal() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act(|act| {
                act.with_id("fn2")
                    .with_name("fn 2")
                    .with_input("uid", json!("b"))
            })
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let s = scher.clone();
    let r = ret.clone();

    let act_id = Arc::new(Mutex::new(None));
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;

            if uid == "a" && e.inner().state == ActionState::Created.to_string() {
                if *count == 0 {
                    *act_id.lock().unwrap() = Some(tid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(uid.to_string()));

                    let action = Action::new(&e.inner().proc_id, tid, "complete", &options);
                    s.do_action(&action).unwrap();
                } else {
                    *r.lock().unwrap() = true;
                    s.close();
                }
                *count += 1;
            } else if uid == "b" && e.inner().state == ActionState::Created.to_string() {
                // cancel the b's task by a
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!("a".to_string()));

                // get the completed act id in previous step
                let act_id = &*act_id.lock().unwrap();
                let aid = act_id.as_deref().unwrap();
                let action = Action::new(&e.inner().proc_id, aid, "cancel", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_back() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act(|act| {
                act.with_id("fn2")
                    .with_name("fn 2")
                    .with_input("uid", json!("b"))
            })
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        let msg = e.inner();
        if msg.is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = msg.inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &msg.id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&msg.proc_id, tid, "complete", &options);
                s.do_action(&action).unwrap();
            } else if uid == "b" {
                if msg.state() == ActionState::Created {
                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!("b".to_string()));
                    options.insert("to".to_string(), json!("step1".to_string()));
                    let action = Action::new(&msg.proc_id, tid, "back", &options);
                    s.do_action(&action).unwrap();
                }
            } else if msg.state() == ActionState::Created && uid == "a" && *count > 0 {
                *r.lock().unwrap() = uid == "a";
                s.close();
            }

            *count += 1;
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_abort() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act(|act| {
                act.with_id("fn2")
                    .with_name("fn 2")
                    .with_input("uid", json!("b"))
            })
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let message = Action::new(&e.proc_id, &e.id, "abort", &options);
            s.do_action(&message).unwrap();
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert!(proc.state().is_abort());
}

#[tokio::test]
async fn sch_act_submit() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state("created") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "submit", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("fn1").get(0).unwrap().action_state(),
        ActionState::Submitted
    );
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_skip() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "skip", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("fn1").get(0).unwrap().state(),
        TaskState::Skip
    );
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_skip_next() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_act(|act| act.with_id("act1").with_input("uid", json!("a")))
        })
        .with_step(|step| step.with_id("step2"));

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "skip", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Skip
    );
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("step2").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_act_error_action() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("fn1").with_input("uid", json!("a")))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));
                options.insert("err_code".to_string(), json!("1"));
                options.insert("err_message".to_string(), json!("biz error"));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "error", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    emitter.on_error(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(proc.task_by_nid("fn1").get(0).unwrap().state().is_error());
    assert!(proc.task_by_nid("step1").get(0).unwrap().state().is_error());
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_error_action_without_err_code() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("fn1").with_input("uid", json!("a")))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "error", &options);
                let result = s.do_action(&action);
                *r.lock().unwrap() = result.is_err();
                e.close();
            }
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_act_not_support_action() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, tid, "not_support", &options);
                *r.lock().unwrap() = s.do_action(&action).is_err();
                s.close();
            }
            *count += 1;
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_next_by_complete_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act(|act| {
                act.with_id("fn2")
                    .with_name("fn 2")
                    .with_input("uid", json!("b"))
            })
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, tid, "complete", &options);
            s.do_action(&action).unwrap();

            // action again
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_by_running_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, tid, "cancel", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_ok();

            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_no_outputs_key_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_output("abc", json!(null))
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();

            // create options that not satisfy the outputs
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_no_uid_key_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_output("abc", json!(null))
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_proc_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new("no_exist_proc_id", &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_msg_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.inner().proc_id, "no_exist_msg_id", "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_not_act_task() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "step" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let s = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s.close();
    });

    (proc, scher, emitter)
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

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let executor = engine.executor().clone();
    let manager = engine.manager().clone();
    let s = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s.close();
    });
    (proc, scher, emitter, executor, manager)
}
