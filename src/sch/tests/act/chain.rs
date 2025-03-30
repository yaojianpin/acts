use crate::event::EventAction;
use crate::{
    sch::tests::create_proc_signal,
    utils::{self, consts},
    Act, StmtBuild, Vars, Workflow,
};

#[tokio::test]
async fn sch_act_chain_list() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#).with_then(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
            })
        })
    });

    main.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut main, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
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
async fn sch_act_chain_order() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#).with_then(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                })
            })
        })
    });

    main.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<i64>>(&mut main, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            rx.update(|data| data.push(e.start_time));
            std::thread::sleep(std::time::Duration::from_secs(1));
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });

    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    let time1 = ret.first().unwrap();
    let time2 = ret.get(1).unwrap();
    assert!(time2 - time1 > 1000);
}

#[tokio::test]
async fn sch_act_chain_var() {
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", ["u1", "u2"])))
            .with_act({
                Act::chain(|act| {
                    act.with_in(r#"$("a")"#).with_then(|stmts| {
                        stmts.add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                    })
                })
            })
    });

    main.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<String>>(&mut main, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
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
async fn sch_act_chain_var_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"$("a")"#)
                    .with_then(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}
