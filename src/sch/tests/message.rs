use crate::event::EventAction;
use crate::{
    data::MessageStatus,
    event::MessageState,
    sch::tests::{create_proc_signal, create_proc_signal2, create_proc_signal_config},
    store::{Cond, Expr},
    utils::{self, consts},
    Act, Action, ChannelOptions, Config, Message, Query, StoreAdapter, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_message_workflow_created() {
    let mut workflow = Workflow::new();
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_workflow_name() {
    let mut workflow = Workflow::new().with_name("my_name");
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<String>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(msg.model.name.clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret, "my_name");
}

#[tokio::test]
async fn sch_message_workflow_tag() {
    let mut workflow = Workflow::new().with_tag("my_tag");
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<String>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(msg.model.tag.clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret, "my_tag");
}

#[tokio::test]
async fn sch_message_workflow_id() {
    let mut workflow = Workflow::new().with_id("my_id");
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<String>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            rx.send(msg.model.id.clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret, "my_id");
}

#[tokio::test]
async fn sch_message_workflow_inputs() {
    let mut workflow = Workflow::new().with_id("my_id").with_input("a", json!(5));
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<Message>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;

    assert_eq!(ret.model.id, "my_id");
    assert_eq!(ret.inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_workflow_outputs() {
    let mut workflow = Workflow::new()
        .with_id("my_id")
        .with_input("a", json!(5))
        .with_output("a", json!(null));
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<Message>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;

    assert_eq!(ret.model.id, "my_id");
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_time() {
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<Vec<bool>>(&mut workflow, &id);

    emitter.on_message(move |msg| {
        if msg.state() == MessageState::Created {
            rx.update(|data| data.push(msg.start_time > 0));
        }

        if msg.state() == MessageState::Completed {
            rx.update(|data| data.push(msg.end_time > 0));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    for v in ret {
        assert!(v);
    }
}

#[tokio::test]
async fn sch_message_step_created() {
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == MessageState::Created {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_step_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!(5))
            .with_output("a", json!(null))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<Message>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "step" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_step_completed() {
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == MessageState::Completed {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_branch_no_message() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_if("false"))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "branch" {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(!ret);
}

#[tokio::test]
async fn sch_message_act_created() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "irq" && e.state() == MessageState::Created {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_created_by_push_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("name", "act 2")
                .with("act", "irq")
                .with("key", "act2")
                .with("tag", "tag2");
            e.do_action(&e.pid, &e.tid, EventAction::Push, &options)
                .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_tag_by_push_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new().with("key", "act2").with("tag", "tag2");
            e.do_action(&e.pid, &e.tid, EventAction::Push, &options)
                .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.send(e.tag == "tag2");
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_inputs_by_push_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("key", "act2")
                .with("with", Vars::new().with("a", 5));
            e.do_action(&e.pid, &e.tid, EventAction::Push, &options)
                .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.send(e.inputs.get::<i32>("a").unwrap() == 5);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_outputs_by_push_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("key", "act2")
                .with("outputs", Vars::new().with("a", 5));
            e.do_action(&e.pid, &e.tid, EventAction::Push, &options)
                .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.send(e.outputs.get::<i32>("a").unwrap() == 5);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_rets_by_push_action() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == MessageState::Created {
            let options = Vars::new()
                .with("key", "act2")
                .with("rets", Vars::new().with("a", json!(null)));
            e.do_action(&e.pid, &e.tid, EventAction::Push, &options)
                .unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            rx.send(
                e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                    .is_err(),
            );
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::irq(|act| {
            act.with_key("act1")
                .with_input("a", json!(5))
                .with_output("a", json!(null))
        }))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<Message>(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.r#type == "irq" && e.state() == MessageState::Created {
            rx.send(e.inner().clone());
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret.outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_act_completed() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "irq" && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }
        if msg.r#type == "irq" && msg.state() == MessageState::Completed {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_sumitted() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Submit, &options);
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == MessageState::Submitted {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Skip, &options);
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == MessageState::Skipped {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_back() {
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::irq(|act| act.with_key("act2")))
        });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("to".to_string(), json!("step1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Back, &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.state() == MessageState::Backed {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_cancel() {
    let act_req_id = Arc::new(Mutex::new(None));
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::irq(|act| act.with_key("act2")))
        });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act1") && msg.is_state(MessageState::Completed) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            *act_req_id.lock().unwrap() = Some(msg.tid.to_string());
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Cancel, &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let act_req_id = &*act_req_id.lock().unwrap();
            let action = Action::new(
                &msg.pid,
                act_req_id.as_deref().unwrap(),
                EventAction::Cancel,
                &options,
            );
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.state() == MessageState::Cancelled {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_remove() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.inner().state() == MessageState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(
                &msg.inner().pid,
                &msg.inner().tid,
                EventAction::Remove,
                &options,
            );
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == MessageState::Removed {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_abort() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.pid, &msg.tid, EventAction::Abort, &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act1") && msg.state() == MessageState::Aborted {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<bool>(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "err1");
            let action = Action::new(&e.pid, &e.tid, EventAction::Error, &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("act1") && e.state() == MessageState::Error {
            rx.send(true);
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[tokio::test]
async fn sch_message_act_inputs_with_err() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vars>(&mut workflow, &utils::longid());

    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.set(consts::ACT_ERR_CODE, "err1");
            options.set(consts::ACT_ERR_MESSAGE, "abc");
            e.do_action(&e.pid, &e.tid, EventAction::Error, &options)
                .unwrap();
        }

        if e.is_key("act1") && e.state() == MessageState::Error {
            rx.send(e.inputs.clone());
        }
    });

    scher.launch(&proc);
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
            .with_act(Act::irq(|act| act.with_key("test")))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vars>(&mut workflow, &utils::longid());

    let step_task_id = Arc::new(Mutex::new("".to_string()));
    let tid = step_task_id.clone();
    emitter.on_message(move |e| {
        if e.is_key("step1") {
            *tid.lock().unwrap() = e.tid.to_string();
        }
        if e.is_source("act") && e.is_state(MessageState::Created) {
            rx.send(e.inputs.clone());
        }
    });

    scher.launch(&proc);
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
    let (engine, proc, tx, rx) = create_proc_signal2::<bool>(&workflow, &id);

    let options = ChannelOptions {
        id: "e1".to_string(),
        ..Default::default()
    };
    engine
        .channel_with_options(&options)
        .on_message(move |msg| {
            if msg.r#type == "workflow" && msg.state() == MessageState::Created {
                rx.send(true);
            }
        });
    engine.runtime().launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_ack_not_exist_message_in_store() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc, tx, rx) = create_proc_signal2::<bool>(&workflow, &id);
    let e2 = engine.clone();
    engine.channel().on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == MessageState::Created {
            let ret = engine.executor().msg().ack(&msg.id);
            rx.send(ret.is_ok());
        }
    });
    e2.runtime().launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_message_ack_exist_message_in_store() {
    let workflow = Workflow::new();
    let id = utils::longid();
    let (engine, proc, tx, rx) = create_proc_signal2::<Message>(&workflow, &id);

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine
        .channel_with_options(&options)
        .on_message(move |msg| {
            if msg.r#type == "workflow" && msg.state() == MessageState::Created {
                engine.executor().msg().ack(&msg.id).unwrap();
                rx.send(msg.inner().clone());
            }
        });
    e2.runtime().launch(&proc);
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
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("act1"))))
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("act2"))));
    let id = utils::longid();
    let (engine, proc, tx, rx) = create_proc_signal2::<String>(&workflow, &id);

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    let e2 = engine.clone();
    engine
        .channel_with_options(&options)
        .on_message(move |msg| {
            if msg.is_key("act1") && msg.state() == MessageState::Created {
                engine
                    .executor()
                    .act()
                    .complete(&msg.pid, &msg.tid, &Vars::new())
                    .unwrap();
                rx.update(|data| *data = msg.id.clone());
            }

            if msg.is_key("act2") && msg.state() == MessageState::Created {
                rx.close();
            }
        });
    e2.runtime().launch(&proc);
    let ret = tx.recv().await;
    let message = e2.runtime().cache().store().messages().find(&ret).unwrap();
    assert_eq!(message.r#type, "irq");
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
    let (engine, proc, tx, rx) = create_proc_signal2::<()>(&workflow, &id);

    let options = ChannelOptions {
        id: "e1".to_string(),
        ack: true,
        ..Default::default()
    };
    engine.channel_with_options(&options).on_complete(move |_| {
        rx.close();
    });
    engine.runtime().launch(&proc);
    tx.recv().await;

    let q = Query::new().push(Cond::and().push(Expr::eq("pid", id)));
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
        Workflow::new().with_step(|step| step.with_act(Act::irq(|act| act.with_key("act1"))));
    let id = utils::longid();
    let (engine, proc, tx, rx) = create_proc_signal2::<Vec<Message>>(&workflow, &id);

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
    engine.runtime().launch(&proc);
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
        Workflow::new().with_step(|step| step.with_act(Act::irq(|act| act.with_key("act1"))));
    let id = utils::longid();

    let config = Config {
        max_message_retry_times: 2,
        ..Config::default()
    };
    let (engine, proc, sig) = create_proc_signal_config::<Vec<Message>>(&config, &workflow, &id);
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
    e2.runtime().launch(&proc);
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
    assert_eq!(message.retry_times, config.max_message_retry_times);
}
