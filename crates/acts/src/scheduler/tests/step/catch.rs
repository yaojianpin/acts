use crate::event::EventAction;
use crate::utils::test::{USES_IRQ, USES_MSG, auto_complete};
use crate::{
    Action, MessageState, Vars, Workflow,
    scheduler::TaskState,
    utils::{self, consts, test::create_proc},
};
use serde_json::json;

#[tokio::test]
async fn sch_step_catch_by_any_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "catch1")))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "aaaaaaaaa");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            s.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });

    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_step_catch_by_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| {
                step.with_id("catch1")
                    .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let channel = engine.channel();
    let (tx, s) = engine.signal(false).double();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "aaaaaaaa");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("msg1") {
            s.send(true);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
    assert_eq!(proc.state(), TaskState::Completed);
}

#[tokio::test]
async fn sch_step_catch_empty_default() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (sig, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set(consts::ACT_ERR_CODE, "err1");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Completed);
}

#[tokio::test]
async fn sch_step_catch_by_err_code() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| {
                step.with_if(r#"$ecode() == "123""#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let emitter = engine.channel();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    emitter.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "123");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_step_catch_by_wrong_code() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| {
                step.with_if(r#"$ecode() == "wrong_code""#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let emitter = engine.channel();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            options.set(consts::ACT_ERR_CODE, "123");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            s.do_action(&action).unwrap();
        }
    });

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_step_catch_by_no_err_code() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "catch1")))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let emitter = engine.channel();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            let state = s.do_action(&action);
            rx.send(state.is_err());
        }
    });

    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret)
}

#[tokio::test]
async fn sch_step_catch_many_if() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("v1", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_catch(|step| {
                step.with_id("catch1")
                    .with_if(r#"v1 > 0"#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
            })
            .with_catch(|step| {
                step.with_id("catch2")
                    .with_if(r#"v1 == 0"#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch2"))
            })
            .with_catch(|step| {
                step.with_id("catch3")
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch3"))
            })
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let emitter = engine.channel();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set(consts::ACT_ERR_CODE, "err1");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            s.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });

    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
    assert_eq!(
        proc.task_by_nid("catch1").first().unwrap().state(),
        TaskState::Running
    );
    assert!(proc.task_by_nid("catch2").is_empty());
    assert!(proc.task_by_nid("catch3").is_empty());
}

#[tokio::test]
async fn sch_step_catch_many_else() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("v1", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_catch(|step| {
                step.with_id("catch_step1")
                    .with_if(r#"v1 < 0"#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
            })
            .with_catch(|step| {
                step.with_id("catch_step2")
                    .with_if(r#"v1 == 0"#)
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch2"))
            })
            .with_catch(|step| {
                step.with_id("catch_step_else")
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch3"))
            })
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set(consts::ACT_ERR_CODE, "err1");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch3") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
    assert_eq!(
        proc.task_by_nid("catch_step1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("catch_step2").first().unwrap().state(),
        TaskState::Skipped
    );
    assert_eq!(
        proc.task_by_nid("catch_step_else").first().unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn sch_step_catch_as_complete() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "catch1")))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let emitter = engine.channel();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    let s = rt.clone();

    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "123");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            s.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
    });

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_params("key", "catch1")
            .first()
            .unwrap()
            .state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_step_catch_as_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "catch1")))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(bool::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    let p = proc.clone();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "1");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "2");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();

            p.print();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn sch_step_catch_as_skip() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_catch(|step| {
                    step.with_id("catch_step1")
                        .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
                })
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "1");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Skip, options);
            rt.do_action(&action).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("catch_step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_params("key", "catch1")
            .first()
            .unwrap()
            .state(),
        TaskState::Skipped
    );
    assert!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .state()
            .is_error()
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
async fn sch_step_catch_as_abort() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "catch1")))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "1");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Abort, options);
            rt.do_action(&action).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Aborted);
}

#[tokio::test]
async fn sch_step_catch_as_submit() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| {
                step.with_id("catch_step1")
                    .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
            })
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "1");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Submit, options);
            rt.do_action(&action).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_params("key", "catch1")
            .first()
            .unwrap()
            .state(),
        TaskState::Submitted
    );
    assert_eq!(
        proc.task_by_nid("catch_step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .state()
            .is_error(),
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[tokio::test]
async fn sch_step_catch_as_back() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_catch(|step| {
                    step.with_id("catch_step1")
                        .with_uses(USES_IRQ, Vars::new().with("key", "catch1"))
                })
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(0).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let count = rx.data();
            if count == 1 {
                rx.close();
                return;
            }

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Next, options);
            rt.do_action(&action).unwrap();
            rx.update(|data| *data += 1);
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "1");
            options.set(consts::ACT_ERR_MESSAGE, "biz error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("catch1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("to".to_string(), json!("step1"));

            let action = Action::new(&e.pid, &e.tid, EventAction::Back, options);
            rt.do_action(&action).unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_params("key", "catch1")
            .first()
            .unwrap()
            .state(),
        TaskState::Backed
    );
    assert_eq!(
        proc.task_by_nid("catch_step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert!(
        proc.task_by_params("key", "act2")
            .first()
            .unwrap()
            .state()
            .is_error()
    );
    assert_eq!(
        proc.task_by_nid("step1").get(1).unwrap().state(),
        TaskState::Running
    );
}

#[tokio::test]
async fn sch_step_catch_and_continue() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_catch(|step| step.with_uses(USES_MSG, Vars::new().with("key", "msg1")))
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (rx, s) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "my_error");

            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            rt.do_action(&action).unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            s.send(true);
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = rx.recv().await;
    proc.print();
    assert!(ret);
}
