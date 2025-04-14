use serde_json::json;

use crate::event::EventAction;
use crate::{
    scheduler::{tests::create_proc_signal, TaskState},
    utils::{self, consts},
    Act, Message, MessageState, StmtBuild, Vars, Workflow,
};

#[tokio::test]
async fn sch_step_acts_msg() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::msg(|msg| msg.with_key("msg1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_acts_req() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|req: crate::Irq| req.with_key("act1")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("irq") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "act1");
}

#[tokio::test]
async fn sch_step_acts_set() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", 10)))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i32>("a")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_step_acts_if_true() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::r#if(|c| {
                c.with_on(r#"$("a") > 0"#).with_then(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
            })
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
    assert_eq!(proc.task_by_nid("act1").len(), 1);
}

#[tokio::test]
async fn sch_step_acts_if_false() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::r#if(|c| {
                c.with_on(r#"$("a") < 0"#)
                    .with_then(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
            })
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
    assert_eq!(proc.task_by_nid("act1").len(), 0);
}

#[tokio::test]
async fn sch_step_acts_each() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::each(|each| {
                each.with_in(r#"["u1", "u2"]"#).with_then(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
            })
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
    let tasks = proc.task_by_nid("act1");
    assert_eq!(tasks.first().unwrap().state(), TaskState::Interrupt);
    assert!(tasks.iter().any(|t| {
        let inputs = t.inputs();
        inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1"
            && inputs.get::<i32>(consts::ACT_INDEX).unwrap() == 0
    }));
    assert!(tasks.iter().any(|t| {
        let inputs = t.inputs();
        inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u2"
            && inputs.get::<i32>(consts::ACT_INDEX).unwrap() == 1
    }));
}

#[tokio::test]
async fn sch_step_acts_chain() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#)
                    .with_then(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rx.update(|data| data.push(e.inputs.get::<String>(consts::ACT_VALUE).unwrap()));
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, ["u1", "u2"]);
}

#[tokio::test]
async fn sch_step_acts_pack() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10)).with_id("step1").with_act({
            Act::block(|act| {
                act.with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1"))))
                    .with_next(|act| act.with_act("msg").with_key("msg2"))
            })
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_type("msg") && e.is_state(MessageState::Created) {
            rx.update(|data| data.push(e.key.clone()));
            // e.do_action(&e.pid, &e.tid, EventAction::Next.as_ref(), &Vars::new())
            //     .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.timeout(100).await;
    proc.print();
    assert_eq!(ret, ["msg1", "msg2"]);
}

#[tokio::test]
async fn sch_step_acts_cmd() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!(10))
            .with_id("step1")
            .with_act(Act::cmd(|act| act.with_key(EventAction::Next.as_ref())))
    });

    workflow.print();
    let (proc, scher, .., tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
}
