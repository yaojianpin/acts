use serde_json::json;

use crate::{
    event::ActionState,
    sch::{tests::create_proc, TaskState},
    utils::{self, consts},
    Act, StmtBuild, Vars, Workflow,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_step_acts_msg() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::msg(|msg| msg.with_id("msg1")))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_acts_req() {
    let acts = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|req: crate::Req| req.with_id("act1")))
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = acts.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let acts = acts.lock().unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts.get(0).unwrap().key, "act1");
}

#[tokio::test]
async fn sch_step_acts_set() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", 10)))
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .data()
            .get::<i32>("a")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_step_acts_expose() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::expose(Vars::new().with("a", 10)))
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<i32>("a").unwrap(), 10);
}

#[tokio::test]
async fn sch_step_acts_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::r#if(|c| {
                c.with_on(r#"env.get("a") > 0"#)
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
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
    assert_eq!(proc.task_by_nid("act1").len(), 1);
}

#[tokio::test]
async fn sch_step_acts_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::r#if(|c| {
                c.with_on(r#"env.get("a") < 0"#)
                    .with_then(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
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
    assert_eq!(proc.task_by_nid("act1").len(), 0);
}

#[tokio::test]
async fn sch_step_acts_each() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::each(|each| {
                each.with_in(r#"["u1", "u2"]"#)
                    .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
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
        proc.task_by_nid("act1")
            .get(0)
            .unwrap()
            .inputs()
            .get_value(consts::ACT_VALUE)
            .unwrap(),
        &json!("u1")
    );
    assert_eq!(
        proc.task_by_nid("act1")
            .get(1)
            .unwrap()
            .inputs()
            .get_value(consts::ACT_VALUE)
            .unwrap(),
        &json!("u2")
    );
}

#[tokio::test]
async fn sch_step_acts_chain() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#)
                    .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            r.lock()
                .unwrap()
                .push(e.inputs.get::<String>(consts::ACT_VALUE).unwrap());
            e.do_action(&e.proc_id, &e.id, consts::EVT_COMPLETE, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), ["u1", "u2"]);
}

#[tokio::test]
async fn sch_step_acts_pack() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::pack(|act| {
                act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
                    .with_next(|act| {
                        act.with_acts(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg2"))))
                    })
            })
        })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_type("msg") {
            r.lock().unwrap().push(e.key.clone());
            e.do_action(&e.proc_id, &e.id, consts::EVT_COMPLETE, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), ["msg1", "msg2"]);
}

#[tokio::test]
async fn sch_step_acts_cmd() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_act(Act::cmd(|act| act.with_name("complete")))
    });

    workflow.print();
    let (proc, scher, ..) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().action_state(),
        ActionState::Completed
    );
}
