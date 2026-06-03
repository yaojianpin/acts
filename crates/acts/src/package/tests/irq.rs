use crate::{
    Act, EngineBuilder, Message, Vars, Workflow,
    event::{Action, EventAction, MessageState},
    scheduler::TaskState,
    utils::{
        self, consts,
        test::{auto_complete, create_proc},
    },
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn pack_irq_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pack_irq_multi_threads() {
    let workflow = Workflow::new().with_id("m1").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    workflow.print();
    let engine = EngineBuilder::new().cache_size(10).build().start().unwrap();
    engine.executor().model().deploy(&workflow).unwrap();
    let (s1, s2) = engine.signal(false).double();
    let count = Arc::new(Mutex::new(0));
    let len = 1000;
    let e2 = engine.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let ret = engine
                .executor()
                .act()
                .complete(&e.pid, &e.tid, Vars::new());
            if ret.is_err() {
                println!("error: {:?}", ret.err().unwrap());
                s1.send(false);
            }

            let mut count = count.lock().unwrap();
            *count += 1;
            // println!("count: {}", *count);
            if *count == len {
                s1.send(true);
            }
        }
    });

    for _ in 0..len {
        e2.executor().proc().start("m1", Vars::new()).unwrap();
    }

    let ret = s2.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_many() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
            .with_act(Act::irq(|act| act.with_key("act2")).with_id("act2"))
            .with_act(Act::irq(|act| act.with_key("act3")).with_id("act3"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_type("act") {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(proc.task_by_nid("act2").len(), 0);
    assert_eq!(proc.task_by_nid("act3").len(), 0);
}

#[tokio::test]
async fn pack_irq_with_inputs_value() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1").with_var("a", 5)).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vars::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            rx.send(e.inputs.clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(ret.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn pack_irq_with_inputs_var() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(5))
            .with_act(Act::irq(|act| {
                act.with_key("act1")
                    .with_params_vars(|vars| vars.with("a", r#"{{ a }}"#))
                    .with_id("act1")
            }))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vars::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            rx.send(e.inputs.clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        ret.get::<Vars>("params").unwrap().get::<i32>("a").unwrap(),
        5
    );
}

#[tokio::test]
async fn pack_irq_complete() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_var("uid", json!("u1"))
        }))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") && e.state() == MessageState::Created {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            if let Err(err) = rt.do_action(&action) {
                println!("error: {err}");
                rx.send(false);
            } else {
                rx.send(true);
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;

    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_cancel_normal() {
    let count = Arc::new(Mutex::new(0));
    let workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_name("step1").with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_var("uid", json!("b"))
            }))
        });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    let act_req_id = Arc::new(Mutex::new(None));
    channel.on_message(move |e| {
        if e.is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;

            if uid == "a" && e.state() == MessageState::Created {
                if *count == 0 {
                    *act_req_id.lock().unwrap() = Some(tid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(uid.to_string()));

                    let action = Action::new(&e.pid, tid, EventAction::Next, options);
                    rt.do_action(&action).unwrap();
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
                let action = Action::new(&e.pid, aid, EventAction::Cancel, options);
                rt.do_action(&action).unwrap();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_back() {
    let count = Arc::new(Mutex::new(0));
    let workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_var("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_var("uid", json!("b"))
            }))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        let msg = e.inner();
        if msg.is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = msg.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &msg.tid;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&msg.pid, tid, EventAction::Next, options);
                rt.do_action(&action).unwrap();
            } else if uid == "b" {
                if msg.state() == MessageState::Created {
                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!("b".to_string()));
                    options.insert("to".to_string(), json!("step1".to_string()));
                    let action = Action::new(&msg.pid, tid, EventAction::Back, options);
                    rt.do_action(&action).unwrap();
                }
            } else if msg.state() == MessageState::Created && uid == "a" && *count > 0 {
                rx.send(uid == "a");
            }

            *count += 1;
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_abort() {
    let workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_var("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_var("uid", json!("b"))
            }))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.is_key("fn1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let message = Action::new(&e.pid, &e.tid, EventAction::Abort, options);
            rt.do_action(&message).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    assert!(proc.state().is_abort());
}

#[tokio::test]
async fn pack_irq_submit() {
    let workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_var("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") && e.is_state(MessageState::Created) {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, EventAction::Submit, options);
                rt.do_action(&action).unwrap();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_skip() {
    let workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_var("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, EventAction::Skip, options);
                rt.do_action(&action).unwrap();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_skip_next() {
    let workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1").with_act(
                Act::irq(|act| act.with_key("act1").with_var("uid", json!("a"))).with_id("act1"),
            )
        })
        .with_step(|step| step.with_id("step2"));

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, EventAction::Skip, options);
                rt.do_action(&action).unwrap();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_error_action() {
    let workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("fn1").with_var("uid", json!("a"))).with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_key("fn1") && e.is_state(MessageState::Created) {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));
                options.set("ecode", "1");
                options.set("error", "biz error");

                let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
                rt.do_action(&action).unwrap();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    assert!(proc.task_by_nid("fn1").first().unwrap().state().is_error());
    assert!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .state()
            .is_error()
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn pack_irq_error_action_without_err_code() {
    let workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_var("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_key("fn1") && e.is_state(MessageState::Created) {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.state() == MessageState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
                let result = rt.do_action(&action);
                rx.update(|data| *data = result.is_err());
                rx.close();
            }
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_next_by_complete_state() {
    let workflow = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| {
            step.with_id("step1")
                .with_name("step1")
                .with_act(Act::irq(|act| {
                    act.with_key("fn1").with_var("uid", json!("a"))
                }))
        })
        .with_step(|step| {
            step.with_name("step2").with_act(Act::irq(|act| {
                act.with_key("fn2").with_var("uid", json!("b"))
            }))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, tid, EventAction::Next, options);
            rt.do_action(&action).unwrap();

            // action again
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_cancel_by_running_state() {
    let workflow = Workflow::new().with_id(&utils::longid()).with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
    });
    let (engine, proc) = create_proc(&workflow, &"w1");
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();
            let tid = &e.tid;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, tid, EventAction::Cancel, options);
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_complete() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "act" {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            let ret = rt.do_action(&action).is_ok();

            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_remove() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.state() == MessageState::Created && e.is_irq() {
            let action = Action::new(&e.pid, &e.tid, EventAction::Remove, Vars::new());
            rt.do_action(&action).unwrap();

            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Removed
    );
}

#[tokio::test]
async fn pack_irq_do_action_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(
            Act::irq(|act| {
                act.with_key("fn1")
                    .with_expose("a", json!(null))
                    .with_expose("b", json!(null))
                    .with_var("uid", json!("a"))
            })
            .with_id("fn1"),
        )
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.is_irq() {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            rt.do_action(&action).unwrap();
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
            .get::<Vec<String>>("c"),
        None
    );
}

#[tokio::test]
async fn pack_irq_do_action_rets() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_name("step1").with_act(
            Act::irq(|act| {
                act.with_key("fn1")
                    .with_expose("a", json!(null))
                    .with_expose("b", json!(null))
                    .with_expose("c", json!(null))
                    .with_var("uid", json!("a"))
            })
            .with_id("fn1"),
        )
    });

    println!("workflow: {workflow:#?}");

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.is_irq() {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("a".to_string(), json!("abc"));
            options.insert("b".to_string(), json!(5));
            options.insert("c".to_string(), json!(["u1", "u2"]));
            options.insert("d".to_string(), json!({ "value": "test" } ));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            rt.do_action(&action).unwrap();
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_do_action_no_output() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.is_irq() {
            let uid = e.inputs.get_value("uid").unwrap().as_str().unwrap();

            // create options that not satisfy the outputs
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));
            options.insert("any".to_string(), json!(100));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            let ret = rt.do_action(&action).is_ok();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_output_key_check() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1")
                    .with_expose("abc", json!(null))
                    .with_var("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.is_irq() {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_proc_id_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.is_irq() {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new("no_exist_proc_id", &e.tid, EventAction::Next, options);
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_msg_id_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1")
            .with_act(Act::irq(|act| {
                act.with_key("fn1").with_var("uid", json!("a"))
            }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, "no_exist_msg_id", EventAction::Next, options);
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_do_action_not_act_req_task() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("fn1").with_var("uid", json!("a"))
        }))
    });

    let pid = utils::longid();
    let (engine, proc) = create_proc(&workflow, &pid);
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.state() == MessageState::Created && e.r#type == "step" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            let ret = rt.do_action(&action).is_err();
            rx.send(ret);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn pack_irq_on_catch_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$ecode() == "err1""#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }
        if e.is_key("act2") {
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn pack_irq_on_catch_as_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$ecode() == "err1""#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Error
    );
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Error
    );
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn pack_irq_on_catch_as_skip() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$ecode() == "err1""#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            rt.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Error
    );
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[tokio::test]
async fn pack_irq_on_catch_no_match() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$ecode() == "err1""#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.task_by_nid("act1").first().unwrap().state().is_error());
    assert!(proc.task_by_nid("act2").is_empty());
}

#[tokio::test]
async fn pack_irq_on_catch_match_any() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| act.with_key("act2")).with_id("act2")),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err2"),
            )
            .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Running
    );
    assert!(
        proc.task_by_nid("act2")
            .first()
            .unwrap()
            .state()
            .is_interrupted()
    );
}

#[tokio::test]
async fn pack_irq_on_catch_as_complete() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_catch(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$ecode() == 'err1'"#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rt.do_action2(
                &e.pid,
                &e.tid,
                EventAction::Error,
                Vars::new().with(consts::ACT_ERR_CODE, "err1"),
            )
            .unwrap();
        }

        if e.is_key("act2") {
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_chain() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
            .with_act(Act::irq(|act| act.with_key("act2")).with_id("act2"))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    let p = proc.clone();
    channel.on_message(move |e| {
        if e.is_key("act1") {
            assert_eq!(
                p.task_by_nid("act1").first().unwrap().state(),
                TaskState::Interrupt
            );
            assert!(p.task_by_nid("act2").is_empty());
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }

        if e.is_key("act2") {
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
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
async fn pack_irq_with_key() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("key1")).with_id("act1"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Message>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
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
async fn pack_irq_on_timeout() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_act(
            Act::irq(|act| act.with_key("act1"))
                .with_id("act1")
                .with_timeout(Act::irq(|act| {
                    act.with_key("act2")
                        .with_id("act2")
                        .with_if(r#"$cost() > 2000"#)
                })),
        )
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act2") {
            rx.close();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}
