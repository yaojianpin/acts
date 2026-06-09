use crate::event::EventAction;
use crate::{
    Action, ChannelOptions, Message, Vars, Workflow,
    config::ConfigData,
    data::MessageStatus,
    event::MessageState,
    store::query::*,
    utils::test::{USES_IRQ, create_proc, create_proc_with_config},
    utils::{self, consts},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_message_workflow_created() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_workflow_name() {
    let workflow = Workflow::new().with_name("my_name");
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(String::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            let name = msg
                .inputs
                .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
                .unwrap()
                .get::<String>("name")
                .unwrap();
            rx.send(name);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret, "my_name");
}

#[tokio::test]
async fn sch_message_workflow_tag() {
    let workflow = Workflow::new().with_tag("my_tag");
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(String::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(
                msg.inputs
                    .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
                    .unwrap()
                    .get::<String>("tag")
                    .unwrap()
                    .clone(),
            );
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret, "my_tag");
}

#[tokio::test]
async fn sch_message_workflow_id() {
    let workflow = Workflow::new().with_id("my_id");
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(String::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            let id = msg
                .inputs
                .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
                .unwrap()
                .get::<String>("id")
                .unwrap();
            rx.send(id);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret, "my_id");
}

#[tokio::test]
async fn sch_message_workflow_inputs() {
    let workflow = Workflow::new().with_id("my_id").with_var("a", json!(5));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(Message::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;

    let id = ret
        .inputs
        .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
        .unwrap()
        .get::<String>("id")
        .unwrap();
    assert_eq!(id, "my_id");
    assert_eq!(ret.inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_workflow_outputs() {
    let workflow = Workflow::new()
        .with_id("my_id")
        .with_var("a", json!(5))
        .with_expose("a", json!(null));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(Message::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;

    let id = ret
        .inputs
        .get::<Vars>(consts::WORKFLOW_MODEL_KEY)
        .unwrap()
        .get::<String>("id")
        .unwrap();
    assert_eq!(id, "my_id");
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_time() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(Vec::<bool>::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    emitter.on_message(move |msg| {
        if msg.state() == MessageState::Created {
            rx.update(|data| data.push(msg.start_time > 0));
        }

        if msg.state() == MessageState::Completed {
            rx.update(|data| data.push(msg.end_time > 0));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    for v in ret {
        assert!(v);
    }
}

#[tokio::test]
async fn sch_message_step_created() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == MessageState::Created {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_step_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(5))
            .with_expose("a", json!(null))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(Message::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "step" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_step_completed() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == MessageState::Completed {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_branch_no_message() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_if("false"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "branch" {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(!ret);
}

#[tokio::test]
async fn sch_message_act_created() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "act" && e.state() == MessageState::Created {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_created_by_push_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("name", "act 2")
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "act2"))
                .with("tag", "tag2");
            rt.do_action2(&e.pid, &e.tid, EventAction::Push, options)
                .unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_tag_by_push_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "act2"))
                .with("tag", "tag2");
            rt.do_action2(&e.pid, &e.tid, EventAction::Push, options)
                .unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            rx.send(e.tag == "tag2");
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_inputs_by_push_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "act2").with("a", 5));
            rt.do_action2(&e.pid, &e.tid, EventAction::Push, options)
                .unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            rx.send(
                e.inputs
                    .get::<Vars>("params")
                    .unwrap()
                    .get::<i32>("a")
                    .unwrap()
                    == 5,
            );
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_outputs_by_push_action() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("uses", "acts.core.irq")
                .with("params", Vars::new().with("key", "act2"))
                .with(
                    "options",
                    Vars::new().with(consts::ACT_EXPOSE, vec![json!({ "name": "a", "value": 5 })]),
                );
            rt.do_action2(&e.pid, &e.tid, EventAction::Push, options)
                .unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            rx.send(e.outputs.get::<i32>("a").unwrap() == 5);
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(5))
            .with_expose("a", json!(null))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(Message::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.r#type == "act" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_act_completed() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.r#type == "act" && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
        if msg.r#type == "act" && msg.state() == MessageState::Completed {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_sumitted() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Submit, options);
            s.do_action(&action).unwrap();
        }
        if msg.is_params_key("act1") && msg.state() == MessageState::Submitted {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_skip() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Skip, options);
            s.do_action(&action).unwrap();
        }
        if msg.is_params_key("act1") && msg.state() == MessageState::Skipped {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_back() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act2") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("to".to_string(), json!("step1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Back, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act2") && msg.state() == MessageState::Backed {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_cancel() {
    let act_req_id = Arc::new(Mutex::new(None));
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act1") && msg.is_state(MessageState::Completed) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            *act_req_id.lock().unwrap() = Some(msg.tid.to_string());
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Cancel, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act2") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let act_req_id = &*act_req_id.lock().unwrap();
            let action = Action::new(
                &msg.pid,
                act_req_id.as_deref().unwrap(),
                EventAction::Cancel,
                options,
            );
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act2") && msg.state() == MessageState::Cancelled {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_remove() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.inner().state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(
                &msg.inner().pid,
                &msg.inner().tid,
                EventAction::Remove,
                options,
            );
            s.do_action(&action).unwrap();
        }
        if msg.is_params_key("act1") && msg.state() == MessageState::Removed {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_abort() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
    emitter.on_message(move |msg| {
        if msg.is_params_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Abort, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act1") && msg.state() == MessageState::Aborted {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
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
        println!("message: {e:?}");
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "err1");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, options);
            s.do_action(&action).unwrap();
        }

        if e.is_params_key("act1") && e.state() == MessageState::Error {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_inputs_with_err() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    workflow.id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "err1");
            options.set(consts::ACT_ERR_MESSAGE, "abc");
            rt.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                .unwrap();
        }

        if e.is_params_key("act1") && e.state() == MessageState::Error {
            rx.send(e.inputs.clone());
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<String>(consts::ACT_ERR_CODE).unwrap(), "err1");
    assert_eq!(ret.get::<String>(consts::ACT_ERR_MESSAGE).unwrap(), "abc");
}

#[tokio::test]
async fn sch_message_act_inputs_with_step_id() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("my step")
            .with_uses(USES_IRQ, Vars::new().with("key", "test"))
    });
    workflow.id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let step_task_id = Arc::new(Mutex::new("".to_string()));
    let tid = step_task_id.clone();
    emitter.on_message(move |e| {
        if e.is_nid("step1") {
            *tid.lock().unwrap() = e.tid.to_string();
        }
        if e.is_type("act") && e.is_state(MessageState::Created) {
            rx.send(e.inputs.clone());
        }
    });

    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(
        ret.get_value(consts::STEP_KEY).unwrap()[consts::STEP_NODE_ID],
        json!("step1")
    );
    assert_eq!(
        ret.get_value(consts::STEP_KEY).unwrap()[consts::STEP_TASK_ID],
        json!(*step_task_id.lock().unwrap())
    );

    assert_eq!(
        ret.get_value(consts::STEP_KEY).unwrap()[consts::STEP_NODE_NAME],
        json!("my step")
    );
}

#[tokio::test]
async fn sch_message_emit_options_with_id() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let chan_options = ChannelOptions {
        id: "e1".to_string(),
        ..Default::default()
    };
    engine
        .channel_with_options(&chan_options)
        .on_message(move |msg| {
            if msg.r#type == "workflow" && msg.state() == MessageState::Created {
                rx.send(true);
            }
        });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_ack_not_exist_message_in_store() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    let e2 = engine.clone();
    engine.channel().on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            let ret = engine.executor().msg().ack(&msg.id);
            rx.send(ret.is_ok());
        }
    });
    e2.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_ack_exist_message_in_store() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(Message::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let chan_options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine
        .channel_with_options(&chan_options)
        .on_message(move |msg| {
            if msg.r#type == "workflow" && msg.state() == MessageState::Created {
                engine.executor().msg().ack(&msg.id).unwrap();
                rx.send(msg.inner().clone());
            }
        });
    e2.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    let message = e2
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&ret.id)
        .unwrap();
    assert_eq!(message.r#type, "workflow");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);
    assert_eq!(message.status, MessageStatus::Acked);
    assert!(message.start_time > 0);
}

#[tokio::test]
async fn sch_message_complete_message_in_store() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")))
        .with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act2")));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(String::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine
        .channel_with_options(&options)
        .on_message(move |msg| {
            if msg.is_params_key("act1") && msg.state() == MessageState::Created {
                engine
                    .executor()
                    .act()
                    .complete(&msg.pid, &msg.tid, Vars::new())
                    .unwrap();
                rx.update(|data| *data = msg.id.clone());
            }

            if msg.is_params_key("act2") && msg.state() == MessageState::Created {
                rx.close();
            }
        });
    e2.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    let message = e2.runtime().cache().store().messages().find(&ret).unwrap();
    assert_eq!(message.r#type, "act");
    assert_eq!(message.uses, "acts.core.irq");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);
    assert_eq!(message.status, MessageStatus::Completed);
    assert!(message.create_time > 0);
    assert!(message.update_time > 0);
}

#[tokio::test]
async fn sch_messages_not_removed_when_completed_in_store() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    engine.channel_with_options(&options).on_complete(move |_| {
        rx.close();
    });
    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;

    let q = Query::new().filter(Filter::and().expr(Expr::eq("pid", id)));
    let messages = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .query(&q)
        .unwrap();
    assert_eq!(messages.count, 1);
}

#[tokio::test]
async fn sch_message_re_sent_if_not_ack() {
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(Vec::<Message>::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    engine.channel_with_options(&options).on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            // not ack the message
            rx.update(|data| data.push(e.inner().clone()));

            if rx.data().len() > 1 {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret.len() > 1);

    let m = ret.first().unwrap();
    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&m.id)
        .unwrap();
    assert_eq!(message.r#type, "workflow");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);
    assert_eq!(message.status, MessageStatus::Created);
    assert!(message.create_time > 0);
    assert!(message.update_time > 0);
    assert!(message.retry_times > 0);
}

#[tokio::test]
async fn sch_message_error_if_not_ack_and_exceed_max_reties() {
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();

    let config = ConfigData {
        max_message_retry_times: Some(2),
        ..ConfigData::default()
    };
    let (engine, proc) = create_proc_with_config(&config, &workflow, &id);
    let _rt = engine.runtime();
    let sig = engine.signal(Vec::<Message>::default());
    let tx = sig.clone();
    let _rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    let rx = sig.clone();
    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine.channel_with_options(&options).on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            // not ack the message
            rx.update(|data| data.push(e.inner().clone()));
        } else {
            engine.executor().msg().ack(&e.id).unwrap();
        }
    });
    e2.runtime().launch(&proc).unwrap();
    let ret = sig.timeout(4000).await;
    assert!(ret.len() > 1);

    let m = ret.first().unwrap();
    let message = e2.runtime().cache().store().messages().find(&m.id).unwrap();
    assert_eq!(message.r#type, "workflow");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);
    assert_eq!(message.status, MessageStatus::Error);
    assert!(message.create_time > 0);
    assert!(message.update_time > 0);
    assert_eq!(message.retry_times, config.max_message_retry_times.unwrap());
}
