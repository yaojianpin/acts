use crate::{
    event::ActionState,
    sch::tests::create_proc,
    utils::{self, consts},
    Act, Action, Message, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_message_workflow_created() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new();
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == ActionState::Created {
            *r.lock().unwrap() = true;
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_workflow_name() {
    let ret = Arc::new(Mutex::new("".to_string()));
    let mut workflow = Workflow::new().with_name("my_name");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == ActionState::Created {
            *r.lock().unwrap() = msg.model.name.clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), "my_name");
}

#[tokio::test]
async fn sch_message_workflow_tag() {
    let ret = Arc::new(Mutex::new("".to_string()));
    let mut workflow = Workflow::new().with_tag("my_tag");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == ActionState::Created {
            *r.lock().unwrap() = msg.model.tag.clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), "my_tag");
}

#[tokio::test]
async fn sch_message_workflow_id() {
    let ret = Arc::new(Mutex::new("".to_string()));
    let mut workflow = Workflow::new().with_id("my_id");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "workflow" && msg.state() == ActionState::Created {
            *r.lock().unwrap() = msg.model.id.clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), "my_id");
}

#[tokio::test]
async fn sch_message_workflow_inputs() {
    let message = Arc::new(Mutex::new(Message::default()));
    let mut workflow = Workflow::new().with_id("my_id").with_input("a", json!(5));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let m = message.clone();
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == ActionState::Created {
            *m.lock().unwrap() = e.inner().clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(message.lock().unwrap().model.id, "my_id");
    assert_eq!(message.lock().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_workflow_outputs() {
    let message = Arc::new(Mutex::new(Message::default()));
    let mut workflow = Workflow::new()
        .with_id("my_id")
        .with_input("a", json!(5))
        .with_output("a", json!(null));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let m = message.clone();
    emitter.on_message(move |e| {
        if e.r#type == "workflow" && e.state() == ActionState::Created {
            *m.lock().unwrap() = e.inner().clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(message.lock().unwrap().model.id, "my_id");
    assert_eq!(message.lock().unwrap().outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_time() {
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.state() == ActionState::Created {
            assert!(msg.start_time > 0);
        }

        if msg.state() == ActionState::Completed {
            assert!(msg.end_time > 0);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_step_created() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == ActionState::Created {
            *r.lock().unwrap() = true;
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_step_outputs() {
    let message = Arc::new(Mutex::new(Message::default()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!(5))
            .with_output("a", json!(null))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let m = message.clone();
    emitter.on_message(move |e| {
        if e.r#type == "step" && e.state() == ActionState::Created {
            *m.lock().unwrap() = e.inner().clone();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(message.lock().unwrap().outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_step_completed() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "step" && msg.state() == ActionState::Completed {
            *r.lock().unwrap() = true;
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_branch_no_message() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_if("false"))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.r#type == "branch" {
            *r.lock().unwrap() = true;
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), false);
}

#[tokio::test]
async fn sch_message_act_created() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.r#type == "req" && e.state() == ActionState::Created {
            *r.lock().unwrap() = true;
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_created_by_push_action() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == ActionState::Created {
            let options = Vars::new()
                .with("id", "act2")
                .with("name", "act 2")
                .with("tag", "tag2");
            e.do_action(&e.proc_id, &e.id, "push", &options).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = true;
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_tag_by_push_action() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == ActionState::Created {
            let options = Vars::new().with("id", "act2").with("tag", "tag2");
            e.do_action(&e.proc_id, &e.id, "push", &options).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = e.tag == "tag2";
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_inputs_by_push_action() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == ActionState::Created {
            let options = Vars::new()
                .with("id", "act2")
                .with("inputs", &Vars::new().with("a", 5));
            e.do_action(&e.proc_id, &e.id, "push", &options).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = e.inputs.get::<i32>("a").unwrap() == 5;
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_outputs_by_push_action() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == ActionState::Created {
            let options = Vars::new()
                .with("id", "act2")
                .with("outputs", &Vars::new().with("a", 5));
            e.do_action(&e.proc_id, &e.id, "push", &options).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = e.outputs.get::<i32>("a").unwrap() == 5;
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_rets_by_push_action() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.r#type == "step" && e.state() == ActionState::Created {
            let options = Vars::new()
                .with("id", "act2")
                .with("rets", &Vars::new().with("a", json!(null)));
            e.do_action(&e.proc_id, &e.id, "push", &options).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = e
                .do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .is_err();
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_outputs() {
    let message = Arc::new(Mutex::new(Message::default()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::req(|act| {
            act.with_id("act1")
                .with_input("a", json!(5))
                .with_output("a", json!(null))
        }))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let m = message.clone();
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.r#type == "req" && e.state() == ActionState::Created {
            *m.lock().unwrap() = e.inner().clone();
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(message.lock().unwrap().outputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_message_act_completed() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.r#type == "req" && msg.state() == ActionState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "complete", &options);
            s.do_action(&action).unwrap();
        }
        if msg.r#type == "req" && msg.state() == ActionState::Completed {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_sumitted() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.state() == ActionState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "submit", &options);
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == ActionState::Submitted {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_skip() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.state() == ActionState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "skip", &options);
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == ActionState::Skipped {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_back() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|act| act.with_id("act2")))
        });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "complete", &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("to".to_string(), json!("step1"));
            let action = Action::new(&msg.proc_id, &msg.id, "back", &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.state() == ActionState::Backed {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_cancel() {
    let ret = Arc::new(Mutex::new(false));
    let act_req_id = Arc::new(Mutex::new(None));
    let mut workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|act| act.with_id("act2")))
        });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "complete", &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act1") && msg.is_state("completed") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            *act_req_id.lock().unwrap() = Some(msg.id.to_string());
            let action = Action::new(&msg.proc_id, &msg.id, "cancel", &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let act_req_id = &*act_req_id.lock().unwrap();
            let action = Action::new(
                &msg.proc_id,
                act_req_id.as_deref().unwrap(),
                "cancel",
                &options,
            );
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act2") && msg.state() == ActionState::Cancelled {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_remove() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.inner().state() == ActionState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.inner().proc_id, &msg.inner().id, "remove", &options);
            s.do_action(&action).unwrap();
        }
        if msg.is_key("act1") && msg.state() == ActionState::Removed {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_abort() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |msg| {
        if msg.is_key("act1") && msg.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.proc_id, &msg.id, "abort", &options);
            s.do_action(&action).unwrap();
        }

        if msg.is_key("act1") && msg.state() == ActionState::Aborted {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("err_code".to_string(), json!("err1"));
            let action = Action::new(&e.proc_id, &e.id, "error", &options);
            s.do_action(&action).unwrap();
        }

        if e.is_key("act1") && e.state() == ActionState::Error {
            *r.lock().unwrap() = true;
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_message_act_inputs_with_step_id() {
    let ret = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("my step")
            .with_act(Act::req(|act| act.with_id("test")))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let step_task_id = Arc::new(Mutex::new("".to_string()));
    let r = ret.clone();
    let tid = step_task_id.clone();
    emitter.on_message(move |e| {
        if e.is_key("step1") {
            *tid.lock().unwrap() = e.id.to_string();
        }
        if e.is_source("act") && e.is_state("created") {
            *r.lock().unwrap() = e.inputs.clone();
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    let inputs = ret.lock().unwrap();
    assert_eq!(
        inputs.get_value(consts::STEP_KEY).clone().unwrap()[consts::STEP_NODE_ID],
        json!("step1")
    );
    assert_eq!(
        inputs.get_value(consts::STEP_KEY).clone().unwrap()[consts::STEP_TASK_ID],
        json!(*step_task_id.lock().unwrap())
    );

    assert_eq!(
        inputs.get_value(consts::STEP_KEY).clone().unwrap()[consts::STEP_NODE_NAME],
        json!("my step")
    );
}
