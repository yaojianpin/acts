use crate::{sch::tests::create_proc, utils, Act, StmtBuild, Workflow};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_step_timeout_one() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|t| {
                t.with_on("1s")
                    .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
            })
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_key("msg1") {
            *r.lock().unwrap() = true;
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap())
}

#[tokio::test]
async fn sch_step_timeout_many() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_timeout(|t| {
                t.with_on("1s")
                    .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
            })
            .with_timeout(|t| {
                t.with_on("2s")
                    .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg2"))))
            })
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.is_key("msg1") {
            r.lock().unwrap().push(e.inner().clone());
        }

        if e.is_key("msg2") {
            r.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let ret = ret.lock().unwrap();
    assert_eq!(ret.len(), 2)
}
