use crate::{
    sch::tests::create_proc,
    utils::{self, consts},
    Act, StmtBuild, Vars, Workflow,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_act_chain_list() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#)
                    .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
        })
    });

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());
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
async fn sch_act_chain_order() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"["u1", "u2"]"#)
                    .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
        })
    });

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            r.lock().unwrap().push(e.start_time);
            std::thread::sleep(std::time::Duration::from_secs(1));
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let times = ret.lock().unwrap();
    let time1 = times.get(0).unwrap();
    let time2 = times.get(1).unwrap();
    assert!(time2 - time1 > 1000);
}

#[tokio::test]
async fn sch_act_chain_var() {
    let ret = Arc::new(Mutex::new(Vec::new()));
    let mut main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", r#"["u1", "u2"]"#)))
            .with_act({
                Act::chain(|act| {
                    act.with_in(r#"env.get("a")"#)
                        .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
                })
            })
    });

    main.print();
    let (proc, scher, emitter) = create_proc(&mut main, &utils::longid());
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
async fn sch_act_chain_var_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act({
            Act::chain(|act| {
                act.with_in(r#"env.get("a")"#)
                    .with_run(|stmts| stmts.add(Act::req(|act| act.with_id("act1"))))
            })
        })
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(proc.state().is_error());
}
