use crate::Config;
use crate::event::EventAction;
use crate::{
    Action, ChannelOptions, Message, Variant, Vars, Workflow,
    config::ConfigData,
    data::MessageStatus,
    event::MessageState,
    store::query::*,
    utils::test::{USES_IRQ, create_proc, create_proc_with_config},
    utils::{self, consts},
};
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;

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
    let workflow = Workflow::new().with_option("tag", "my_tag");
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
                    .get::<Vars>("options")
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
        .with_expose(Variant::create("a", json!(null)));
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
            .with_expose(Variant::create("a", json!(null)))
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
async fn sch_message_step_tag() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_option("tag", "my_step_tag"));
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
        if msg.r#type == "step" && msg.state() == MessageState::Created {
            let tag = msg
                .options()
                .and_then(|o| o.get::<String>("tag"))
                .unwrap_or_default();
            rx.send(tag);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret, "my_step_tag");
}

#[tokio::test]
async fn sch_message_act_tag() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_option("tag", "my_act_tag")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
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
        if msg.r#type == "act" && msg.state() == MessageState::Created {
            let tag = msg
                .options()
                .and_then(|o| o.get::<String>("tag"))
                .unwrap_or_default();
            rx.send(tag);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret, "my_act_tag");
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
                .with("options", Vars::new().with("tag", "tag2"));
            rt.do_action2(&e.pid, &e.tid, EventAction::Push, options)
                .unwrap();
        }

        if e.is_params_key("act2") && e.is_state(MessageState::Created) {
            rx.send(
                e.options()
                    .and_then(|o| o.get::<String>("tag"))
                    .unwrap_or_default()
                    == "tag2",
            );
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
                    Vars::new().with("exposes", vec![json!({ "name": "a", "value": 5 })]),
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
            .with_expose(Variant::create("a", json!(null)))
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

            *act_req_id.lock() = Some(msg.tid.to_string());
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Cancel, options);
            s.do_action(&action).unwrap();
        }

        if msg.is_params_key("act2") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let act_req_id = &*act_req_id.lock();
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
            *tid.lock() = e.tid.to_string();
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
        json!(*step_task_id.lock())
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
                // the channel delivery carries its own delivery id
                let delivery_id = msg.delivery_id.clone().unwrap();
                engine.executor().msg().ack(&delivery_id).unwrap();
                rx.send(msg.inner().clone());
            }
        });
    e2.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;

    // the canonical message is stored once, keyed by the message id
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
    assert!(message.start_time > 0);

    // the delivery row records the ack of this channel
    let delivery_id = ret.delivery_id.clone().unwrap();
    let delivery = e2
        .runtime()
        .cache()
        .store()
        .deliveries()
        .find(&delivery_id)
        .unwrap();
    assert_eq!(delivery.chan_id, "e1");
    assert_eq!(delivery.status, MessageStatus::Acked);
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
    e2.runtime().cache().flush().unwrap();

    // canonical message row (payload is stored once per message id)
    let message = e2.runtime().cache().store().messages().find(&ret).unwrap();
    assert_eq!(message.r#type, "act");
    assert_eq!(message.uses.unwrap_or_default(), "acts.core.irq");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);
    assert!(message.create_time > 0);

    // the delivery row is completed when the task completes
    let delivery_id = e2
        .runtime()
        .cache()
        .store()
        .deliveries()
        .query(&Query::new().filter(Filter::and().expr(Expr::eq("msg_id", ret.clone()))))
        .unwrap()
        .rows
        .first()
        .unwrap()
        .id
        .clone();
    let delivery = e2
        .runtime()
        .cache()
        .store()
        .deliveries()
        .find(&delivery_id)
        .unwrap();
    assert_eq!(delivery.status, MessageStatus::Completed);
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
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

    // every redelivery carries the same delivery id of the stored row
    assert_eq!(ret[0].delivery_id, ret[1].delivery_id);
    assert!(ret[0].delivery_id.is_some());

    let m = ret.first().unwrap();
    // canonical message row — stored once per message id
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

    // the delivery row keeps the retry state of this channel
    let delivery = engine
        .runtime()
        .cache()
        .store()
        .deliveries()
        .find(&m.delivery_id.clone().unwrap())
        .unwrap();
    assert_eq!(delivery.status, MessageStatus::Created);
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
    assert!(delivery.retry_times > 0);
}

#[tokio::test]
async fn sch_message_error_if_not_ack_and_exceed_max_reties() {
    let workflow =
        Workflow::new().with_step(|step| step.with_uses(USES_IRQ, Vars::new().with("key", "act1")));
    let id = utils::longid();

    let (engine, proc) = create_proc_with_config(
        &Config {
            data: ConfigData {
                max_message_retry_times: Some(2),
                ..ConfigData::default()
            },
            ..Default::default()
        },
        &workflow,
        &id,
    );
    let config = engine.config();
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
        } else if let Some(delivery_id) = &e.delivery_id {
            // ack the other deliveries of this channel
            engine.executor().msg().ack(delivery_id).unwrap();
        }
    });
    e2.runtime().launch(&proc).unwrap();
    let ret = sig.timeout(4000).await;
    assert!(ret.len() > 1);

    let m = ret.first().unwrap();
    // canonical message row
    let message = e2.runtime().cache().store().messages().find(&m.id).unwrap();
    assert_eq!(message.r#type, "workflow");
    assert_eq!(message.pid, id);
    assert_eq!(message.state, MessageState::Created);

    // the delivery row turns into error after max retries
    let delivery = e2
        .runtime()
        .cache()
        .store()
        .deliveries()
        .find(&m.delivery_id.clone().unwrap())
        .unwrap();
    assert_eq!(delivery.status, MessageStatus::Error);
    assert!(delivery.create_time > 0);
    assert!(delivery.update_time > 0);
    assert_eq!(delivery.retry_times, config.max_message_retry_times());
}

#[tokio::test]
async fn sch_message_redelivery_goes_to_owning_channel_only() {
    // two ack channels share the same emitted messages; channel a acks its
    // deliveries while channel b does not — the retry timer must re-send only
    // channel b's deliveries, never acked channel a again
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let _rt = engine.runtime();

    let sig_b = engine.signal(Vec::<Message>::default());
    let b_send = sig_b.clone();
    let b_close = sig_b.clone();
    let sig_a = engine.signal(Vec::<Message>::default());
    let a_send = sig_a.clone();
    let a_recv = sig_a.clone();

    let engine_a = engine.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_a".to_string(),
            ack: true,
            ..Default::default()
        })
        .on_message(move |e| {
            if let Some(delivery_id) = &e.delivery_id {
                engine_a.executor().msg().ack(delivery_id).unwrap();
            }
            if e.r#type == "workflow" && e.state() == MessageState::Created {
                a_send.update(|data| data.push(e.inner().clone()));
            }
        });

    // channel b: never acks, records the workflow-created redeliveries
    let b_close2 = b_close.clone();
    engine
        .channel_with_options(&ChannelOptions {
            id: "chan_b".to_string(),
            ack: true,
            ..Default::default()
        })
        .on_message(move |e| {
            if e.r#type == "workflow" && e.state() == MessageState::Created {
                b_send.update(|data| data.push(e.inner().clone()));
                if b_close2.data().len() > 1 {
                    b_close2.close();
                }
            }
        });

    engine.runtime().launch(&proc).unwrap();
    let received_b = sig_b.timeout(6000).await;
    assert!(
        received_b.len() > 1,
        "channel b should receive the workflow-created message again, got {:?}",
        received_b.len()
    );

    // both redeliveries are the same delivery of the same message
    assert_eq!(received_b[0].id, received_b[1].id);
    assert_eq!(received_b[0].delivery_id, received_b[1].delivery_id);
    let msg_id = received_b[0].id.clone();

    // channel a saw the message exactly once (its ack stopped the retries)
    let received_a = a_recv.timeout(200).await;
    assert_eq!(received_a.len(), 1, "channel a must not be redelivered");
    assert_eq!(received_a[0].id, msg_id);
}

#[tokio::test]
async fn sch_message_channel_options_match() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_option("tag", "important"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    let emitter = engine.channel_with_options(&ChannelOptions {
        options: Vars::new().with("tag", "important"),
        ..Default::default()
    });
    let tx_close = sig.clone();
    let tx_close2 = tx_close.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
    });
    rt.launch(&proc).unwrap();
    let ret = sig.timeout(500).await;
    // should receive step created + step completed + workflow created + workflow completed
    // all have options.tag = "important"
    assert!(
        ret.len() >= 2,
        "expected at least 2 messages, got {}",
        ret.len()
    );
    for msg in &ret {
        let tag = msg
            .options()
            .and_then(|o| o.get::<String>("tag"))
            .unwrap_or_default();
        assert_eq!(tag, "important");
    }
}
