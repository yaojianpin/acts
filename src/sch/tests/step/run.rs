use crate::{
    sch::tests::create_proc_signal2,
    utils::{self, consts},
    Act, Event, Message, Signal, StmtBuild, Workflow,
};
use serde_json::json;

#[tokio::test]
async fn sch_step_run_msg() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.msg({ key: "msg1" })"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_req() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.req({ key: "act1" })"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("act1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_chain_array() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.chain({ in: '["u1"]', run: [ { msg: { key: "msg1" } } ] })"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_chain_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| {
            step.with_id("step1")
                .with_run(r#"act.chain({ in: $("a"), run: [ { msg: { key: "msg1" } } ] })"#)
        });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_each_array() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.each({ in: '["u1"]', run: [ { msg: { key: "msg1" } } ] })"#)
    });

    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_each_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| {
            step.with_id("step1")
                .with_run(r#"act.each({ in: $("a"), run: [ { msg: { key: "msg1" } } ] })"#)
        });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_block() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.block({ acts: [{ msg: { id: "msg1" } }] })"#)
    });

    let ret = run_test(&workflow, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_call() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_run(r#"act.call({ mid: "m1" })"#));
    let dep = Workflow::new().with_id("m1");
    let ret = run_test_dep(&workflow, &dep, |e, s| {
        if e.is_key("m1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_push() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.push({ req: { id: "act1" } })"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("act1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_set_output() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#" act.set_output("a", 100);"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<i32>("a").unwrap() == 100);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_abort() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_run(r#"act.abort();"#));
    let ret: bool = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("aborted") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_fail() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_run(r#"act.fail("err1", "error message");"#)
    });
    let ret: bool = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("error") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_skip() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_run(r#"act.skip();"#));

    let ret: bool = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("skipped") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_back() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2").with_run(r#"act.back("step1");"#));
    let ret: bool = run_test(&workflow, |e, s| {
        if e.is_key("step2") && e.is_state("backed") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_state() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_run(
            r#"
    let state = act.state();
    act.set_output("state", state);
    "#,
        )
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<String>("state").unwrap() == "running");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_set_value() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_run(r#"act.set("my_data", "abc");"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<String>("my_data").unwrap() == "abc");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_throw_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_run(r#" throw new Error("test error");"#)
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("error") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_run_catch_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_run(r#" throw new Error("test error");"#)
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::msg(|act| act.with_id("msg1")))))
    });
    let ret = run_test(&workflow, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

async fn run_test<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    let s = rx.clone();
    engine.emitter().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });

    engine.emitter().on_error(move |_| {
        s.close();
    });
    engine.runtime().launch(&proc);
    tx.recv().await
}

async fn run_test_dep<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    dep: &Workflow,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    engine.manager().deploy(&dep).unwrap();
    engine.emitter().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });
    engine.runtime().launch(&proc);
    tx.recv().await
}
