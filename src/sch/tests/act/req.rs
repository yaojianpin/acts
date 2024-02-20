use crate::{
    event::{Action, ActionState},
    sch::{
        tests::{create_proc, create_proc2},
        TaskState,
    },
    utils, Act, StmtBuild, Vars, Workflow,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn sch_act_req_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::req(|act| act.with_id("act1"))))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
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
async fn sch_act_req_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::req(|act| act.with_id("act1")))
                .add(Act::req(|act| act.with_id("act2")))
                .add(Act::req(|act| act.with_id("act3")))
        })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
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
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act3").get(0).unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_req_with_inputs_value() {
    let ret = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::req(|act| act.with_id("act1").with_input("a", 5))))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") && e.is_state("created") {
            *r.lock().unwrap() = e.inputs.clone();
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
    assert_eq!(ret.lock().unwrap().get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_act_req_with_inputs_var() {
    let ret = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_input("a", json!(5)).with_act({
            Act::req(|act| act.with_id("act1").with_input("a", r#"${ env.get("a") }"#))
        })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") && e.is_state("created") {
            *r.lock().unwrap() = e.inputs.clone();
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
    assert_eq!(ret.lock().unwrap().get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_act_req_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act({
            Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("u1"))
            })
        })
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let r = ret.clone();
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            if e.state() == ActionState::Created {
                let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, &e.id, "complete", &options);
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
async fn sch_act_req_cancel_normal() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_name("step1").with_act({
                Act::req(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act({
                Act::req(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let s = scher.clone();
    let r = ret.clone();

    let act_req_id = Arc::new(Mutex::new(None));
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.id;

            if uid == "a" && e.state == ActionState::Created.to_string() {
                if *count == 0 {
                    *act_req_id.lock().unwrap() = Some(tid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(uid.to_string()));

                    let action = Action::new(&e.proc_id, tid, "complete", &options);
                    s.do_action(&action).unwrap();
                } else {
                    *r.lock().unwrap() = true;
                    s.close();
                }
                *count += 1;
            } else if uid == "b" && e.state == ActionState::Created.to_string() {
                // cancel the b's task by a
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!("a".to_string()));

                // get the completed act id in previous step
                let act_req_id = &*act_req_id.lock().unwrap();
                let aid = act_req_id.as_deref().unwrap();
                let action = Action::new(&e.proc_id, aid, "cancel", &options);
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
async fn sch_act_req_back() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_name("step1").with_act({
                Act::req(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act({
                Act::req(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        let msg = e.inner();
        if msg.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = msg.inputs.get_value("uid").unwrap().as_str().unwrap();
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
async fn sch_act_req_abort() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_name("step1").with_act({
                Act::req(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
        })
        .with_step(|step| {
            step.with_name("step2").with_act({
                Act::req(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
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
async fn sch_act_req_submit() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, &e.id, "submit", &options);
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
async fn sch_act_req_skip() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act({
            Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, &e.id, "skip", &options);
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
async fn sch_act_req_skip_next() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_act(Act::req(|act| {
                act.with_id("act1").with_input("uid", json!("a"))
            }))
        })
        .with_step(|step| step.with_id("step2"));

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, &e.id, "skip", &options);
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
async fn sch_act_req_error_action() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(Act::req(|act| {
            act.with_id("fn1").with_input("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));
                options.insert("err_code".to_string(), json!("1"));
                options.insert("err_message".to_string(), json!("biz error"));

                let action = Action::new(&e.proc_id, &e.id, "error", &options);
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
async fn sch_act_req_error_action_without_err_code() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(Act::req(|act| {
            act.with_id("fn1").with_input("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, &e.id, "error", &options);
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
async fn sch_act_req_not_support_action() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_name("step1").with_act({
            Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.proc_id, tid, "not_support", &options);
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
async fn sch_act_req_next_by_complete_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::req(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::req(|act| {
                act.with_id("fn2")
                    .with_name("fn 2")
                    .with_input("uid", json!("b"))
            }))
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.proc_id, tid, "complete", &options);
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
async fn sch_act_req_cancel_by_running_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            }))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.proc_id, tid, "cancel", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_req_do_action_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_ok();

            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_req_do_action_remove() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.state() == ActionState::Created && e.r#type == "req" {
            let action = Action::new(&e.proc_id, &e.id, "remove", &Vars::new());
            s.do_action(&action).unwrap();

            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().state(),
        TaskState::Removed
    );
}

#[tokio::test]
async fn sch_act_req_do_action_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_output("a", json!(null))
                    .with_output("b", json!(null))
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            s.do_action(&action).unwrap();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<i32>("b")
            .unwrap(),
        5
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<Vec<String>>("c"),
        None
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .get(0)
            .unwrap()
            .env()
            .get::<Vec<String>>("c")
            .unwrap(),
        ["u1", "u2"]
    );
}

#[tokio::test]
async fn sch_act_req_do_action_rets() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_ret("a", json!(null))
                    .with_ret("b", json!(null))
                    .with_ret("c", json!(null))
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));
            options.insert("d".to_string(), json!({ "value": "test" } ));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            s.do_action(&action).unwrap();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("fn1")
            .get(0)
            .unwrap()
            .env()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .get(0)
            .unwrap()
            .env()
            .get::<i32>("b")
            .unwrap(),
        5
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .get(0)
            .unwrap()
            .env()
            .get::<Vec<String>>("c")
            .unwrap(),
        ["u1", "u2"]
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .get(0)
            .unwrap()
            .env()
            .get::<Value>("d"),
        None
    );
}

#[tokio::test]
async fn sch_act_req_do_action_no_rets() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            // create options that not satisfy the outputs
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("any".to_string(), json!(100));

            let action = Action::new(&e.proc_id, &e.id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_ok();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_req_do_action_ret_key_check() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_ret("abc", json!(null))
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
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

#[tokio::test]
async fn sch_act_req_do_action_proc_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new("no_exist_proc_id", &e.id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_req_do_action_msg_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::req(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "req" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.proc_id, "no_exist_msg_id", "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_req_do_action_not_act_req_task() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::req(|act| {
            act.with_id("fn1")
                .with_name("fn 1")
                .with_input("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.state() == ActionState::Created && e.r#type == "step" {
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

#[tokio::test]
async fn sch_act_req_on_created_msg() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_on_created(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(messages.lock().unwrap().get(0).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_req_on_created_act() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_on_created(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_source("act") {
            m.lock().unwrap().push(e.inner().clone());

            if e.is_key("act2") {
                e.close();
            }
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(messages.lock().unwrap().get(0).unwrap().key, "act1");
    assert_eq!(messages.lock().unwrap().get(1).unwrap().key, "act2");
}

#[tokio::test]
async fn sch_act_req_on_completed_msg() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_on_completed(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(messages.lock().unwrap().get(0).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_req_on_completed_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_on_completed(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }

        if e.is_key("act2") {
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
}

#[tokio::test]
async fn sch_act_req_on_catch() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
            })
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
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
}

#[tokio::test]
async fn sch_act_req_on_catch_as_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
            })
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err2"),
            )
            .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().action_state(),
        ActionState::Error
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_req_on_catch_as_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
            })
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(&e.proc_id, &e.id, "skip", &Vars::new())
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().action_state(),
        ActionState::Skipped
    );
}

#[tokio::test]
async fn sch_act_req_on_catch_no_match() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
            })
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err2"),
            )
            .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(proc.task_by_nid("act1").get(0).unwrap().state().is_error());
    assert!(proc.task_by_nid("act2").get(0).is_none());
}

#[tokio::test]
async fn sch_act_req_on_catch_match_any() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_catch(|c| c.with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2")))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err2"),
            )
            .unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
    assert!(proc
        .task_by_nid("act2")
        .get(0)
        .unwrap()
        .state()
        .is_interrupted());
}

#[tokio::test]
async fn sch_act_req_on_catch_as_complete() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act2"))))
            })
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.proc_id,
                &e.id,
                "error",
                &Vars::new().with("err_code", "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").get(0).unwrap().action_state(),
        ActionState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
}

#[tokio::test]
async fn sch_act_req_chain() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_on_completed(|stmts| stmts.add(Act::req(|req| req.with_id("act2"))))
        }))
    });

    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let p = proc.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") {
            assert_eq!(
                p.task_by_nid("act1").get(0).unwrap().state(),
                TaskState::Interrupt
            );
            assert!(p.task_by_nid("act2").get(0).is_none());
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }

        if e.is_key("act2") {
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
        TaskState::Success
    );
}
