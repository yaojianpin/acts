use crate::{
    event::{Action, MessageState},
    sch::{tests::*, TaskState},
    utils::{self, consts},
    Act, Message, StmtBuild, Vars, Workflow,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn sch_act_irq_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::irq(|act| act.with_key("act1")).with_id("act1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sch_act_irq_multi_threads() {
    let workflow = Workflow::new().with_id("m1").with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::irq(|act| act.with_key("act1"))))
    });

    workflow.print();
    let engine = Builder::new().cache_size(10).build();
    engine.executor().model().deploy(&workflow).unwrap();
    let (s1, s2) = engine.signal(false).double();
    let count = Arc::new(Mutex::new(0));
    let len = 1000;
    let e2 = engine.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let ret = engine
                .executor()
                .act()
                .complete(&e.pid, &e.tid, &Vars::new());
            if ret.is_err() {
                println!("error: {:?}", ret.err().unwrap());
                s1.send(false);
            }

            let mut count = count.lock().unwrap();
            *count += 1;
            println!("count: {}", *count);
            if *count == len {
                s1.send(true);
            }
        }
    });

    for _ in 0..len {
        e2.executor().proc().start("m1", &Vars::new()).unwrap();
    }

    let ret = s2.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                .add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                .add(Act::irq(|act| act.with_key("act3")).with_id("act3"))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act3").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_irq_with_inputs_value() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup.add(Act::irq(|act| act.with_key("act1").with_input("a", 5)).with_id("act1"))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("irq") && e.is_state("created") {
            rx.send(e.inputs.clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(ret.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_act_irq_with_inputs_var() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_input("a", json!(5)).with_act(
            Act::irq(|act| act.with_key("act1").with_input("a", r#"${ $("a") }"#)).with_id("act1"),
        )
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("irq") && e.is_state("created") {
            rx.send(e.inputs.clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(ret.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_act_irq_complete() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_input("uid", json!("u1"))
        }))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") && e.state() == MessageState::Created {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            if let Err(err) = s.do_action(&action) {
                println!("error: {}", err);
                rx.send(false);
            } else {
                rx.send(true);
            }
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;

    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_cancel_normal() {
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_name("step1").with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_input("uid", json!("b"))
            }))
        });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());
    let s = scher.clone();
    let act_req_id = Arc::new(Mutex::new(None));
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;

            if uid == "a" && e.state() == MessageState::Created {
                if *count == 0 {
                    *act_req_id.lock().unwrap() = Some(tid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(uid.to_string()));

                    let action = Action::new(&e.pid, tid, consts::EVT_NEXT, &options);
                    s.do_action(&action).unwrap();
                } else {
                    rx.send(true);
                }
                *count += 1;
            } else if uid == "b" && e.state() == MessageState::Created {
                // cancel the b's task by a
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!("a".to_string()));

                // get the completed act id in previous step
                let act_req_id = &*act_req_id.lock().unwrap();
                let aid = act_req_id.as_deref().unwrap();
                let action = Action::new(&e.pid, aid, "cancel", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_back() {
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_input("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_input("uid", json!("b"))
            }))
        });
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        let msg = e.inner();
        if msg.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = msg.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &msg.tid;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&msg.pid, tid, consts::EVT_NEXT, &options);
                s.do_action(&action).unwrap();
            } else if uid == "b" {
                if msg.state() == MessageState::Created {
                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!("b".to_string()));
                    options.insert("to".to_string(), json!("step1".to_string()));
                    let action = Action::new(&msg.pid, tid, "back", &options);
                    s.do_action(&action).unwrap();
                }
            } else if msg.state() == MessageState::Created && uid == "a" && *count > 0 {
                rx.send(uid == "a");
            }

            *count += 1;
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_abort() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_input("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_input("uid", json!("b"))
            }))
        });
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let message = Action::new(&e.pid, &e.tid, "abort", &options);
            s.do_action(&message).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert!(proc.state().is_abort());
}

#[tokio::test]
async fn sch_act_irq_submit() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_input("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, "submit", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("fn1").first().unwrap().state(),
        TaskState::Submitted
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_irq_skip() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_input("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, "skip", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("fn1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_irq_skip_next() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_act(
                Act::irq(|act| act.with_key("act1").with_input("uid", json!("a"))).with_id("act1"),
            )
        })
        .with_step(|step| step.with_id("step2"));

    let pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, "skip", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("step2").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_irq_error_action() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_input("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));
                options.set("ecode", "1");
                options.set("error", "biz error");

                let action = Action::new(&e.pid, &e.tid, "error", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert!(proc.task_by_nid("fn1").first().unwrap().state().is_error());
    assert!(proc
        .task_by_nid("step1")
        .first()
        .unwrap()
        .state()
        .is_error());
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_irq_error_action_without_err_code() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_input("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("fn1") && e.is_state("created") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, "error", &options);
                let result = s.do_action(&action);
                rx.update(|data| *data = result.is_err());
                rx.close();
            }
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_not_support_action() {
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, tid, "not_support", &options);
                let ret = s.do_action(&action).is_err();
                rx.send(ret);
            }
            *count += 1;
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_next_by_complete_state() {
    let mut workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_input("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_input("uid", json!("b"))
            }))
        });
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, tid, consts::EVT_NEXT, &options);
            s.do_action(&action).unwrap();

            // action again
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_cancel_by_running_state() {
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, "w1");

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_source("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, tid, "cancel", &options);
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_complete() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_ok();

            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_remove() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.state() == MessageState::Created && e.r#type == "irq" {
            let action = Action::new(&e.pid, &e.tid, "remove", &Vars::new());
            s.do_action(&action).unwrap();

            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Removed
    );
}

#[tokio::test]
async fn sch_act_irq_do_action_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(
            Act::irq(|act| {
                act.with_key("fn1")
                    .with_output("a", json!(null))
                    .with_output("b", json!(null))
                    .with_input("uid", json!("a"))
            })
            .with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));

            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            s.do_action(&action).unwrap();
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("b")
            .unwrap(),
        5
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<Vec<String>>("c"),
        None
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<Vec<String>>("c")
            .unwrap(),
        ["u1", "u2"]
    );
}

#[tokio::test]
async fn sch_act_irq_do_action_rets() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(
            Act::irq(|act| {
                act.with_key("fn1")
                    .with_ret("a", json!(null))
                    .with_ret("b", json!(null))
                    .with_ret("c", json!(null))
                    .with_input("uid", json!("a"))
            })
            .with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));
            options.insert("d".to_string(), json!({ "value": "test" } ));

            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            s.do_action(&action).unwrap();
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<i32>("b")
            .unwrap(),
        5
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<Vec<String>>("c")
            .unwrap(),
        ["u1", "u2"]
    );
    assert_eq!(
        proc.task_by_nid("fn1")
            .first()
            .unwrap()
            .data()
            .get::<Value>("d"),
        None
    );
}

#[tokio::test]
async fn sch_act_irq_do_action_no_rets() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            // create options that not satisfy the outputs
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("any".to_string(), json!(100));

            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_ok();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_ret_key_check() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1")
                    .with_ret("abc", json!(null))
                    .with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_proc_id_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new("no_exist_proc_id", &e.tid, consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_msg_id_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_input("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "irq" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, "no_exist_msg_id", consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_do_action_not_act_req_task() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_input("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &pid);

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "step" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, &e.tid, consts::EVT_NEXT, &options);
            let ret = s.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_irq_on_created_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_setup(|stmts| {
                stmts.add(Act::on_created(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
            }))
    });

    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.is_type("msg") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_irq_on_created_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_setup(|stmts| {
                stmts.add(Act::on_created(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act2")))
                }))
            }))
    });

    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_source("act") {
            rx.update(|data| data.push(e.inner().clone()));
            if e.is_key("act2") {
                rx.close();
            }
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.first().unwrap().key, "act1");
    assert_eq!(ret.get(1).unwrap().key, "act2");
}

#[tokio::test]
async fn sch_act_irq_on_completed_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_setup(|stmts| {
                stmts.add(Act::on_completed(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
            }))
    });

    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.pid, &e.tid, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_act_irq_on_completed_act() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_completed(|stmts| {
                        stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(&e.pid, &e.tid, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }

        if e.is_key("act2") {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_irq_on_catch() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_on("err1").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                consts::EVT_ERR,
                &Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }
        if e.is_key("act2") {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_act_irq_on_catch_as_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_on("err1").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Error
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_act_irq_on_catch_as_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_on("err1").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(&e.pid, &e.tid, "skip", &Vars::new()).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[tokio::test]
async fn sch_act_irq_on_catch_no_match() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_on("err1").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.task_by_nid("act1").first().unwrap().state().is_error());
    assert!(proc.task_by_nid("act2").first().is_none());
}

#[tokio::test]
async fn sch_act_irq_on_catch_match_any() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
    assert!(proc
        .task_by_nid("act2")
        .first()
        .unwrap()
        .state()
        .is_interrupted());
}

#[tokio::test]
async fn sch_act_irq_on_catch_as_complete() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_catch(|stmts| {
                        stmts.with(|c| {
                            c.with_on("err1").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            e.do_action(
                &e.pid,
                &e.tid,
                "error",
                &Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            e.do_action(&e.pid, &e.tid, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_irq_chain() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_completed(|stmts| {
                        stmts.add(Act::irq(|req| req.with_key("act2")).with_id("act2"))
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let p = proc.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") {
            assert_eq!(
                p.task_by_nid("act1").first().unwrap().state(),
                TaskState::Interrupt
            );
            assert!(p.task_by_nid("act2").first().is_none());
            e.do_action(&e.pid, &e.tid, consts::EVT_NEXT, &Vars::new())
                .unwrap();
        }

        if e.is_key("act2") {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_act_irq_with_key() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::irq(|act| act.with_key("key1")).with_id("act1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("irq") && e.is_state("created") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "key1");
}

#[tokio::test]
async fn sch_act_irq_on_timeout() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_setup(|stmts| {
                    stmts.add(Act::on_timeout(|stmts| {
                        stmts.with(|c| {
                            c.with_on("2s").with_then(|stmts| {
                                stmts.add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
                            })
                        })
                    }))
                }),
        )
    });

    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act2") {
            rx.close();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}
