use serde_json::json;

use crate::{
    data,
    sch::tests::create_proc_signal2,
    utils::{self, consts},
    Act, Event, Message, Signal, StmtBuild, Workflow,
};

#[tokio::test]
async fn sch_step_uses_package_normal_id() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.msg({ key: "msg1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_package_special_id() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_uses("@aaaaa/bbbb-2.0"));
    let pack = data::Package {
        id: "@aaaaa/bbbb-2.0".to_string(),
        name: "package 1".to_string(),
        data: br#"act.msg({ key: "msg1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_not_exists() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("not_exists"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.msg({ key: "msg1" })"#.to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |_, _| {}).await;
    assert!(!ret);
}

#[tokio::test]
async fn sch_step_uses_msg() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.msg({ key: "msg1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_req() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.irq({ key: "act1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("act1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_chain_array() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.chain({ in: '["u1"]', then: [ { act: "msg", key: "msg1" }] })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_chain_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.chain({ in: $("a"), then: [ { act: "msg", key: "msg1" }] })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_each_array() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.each({ in: '["u1"]', then: [ { act: "msg", key: "msg1" }] })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_each_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.each({ in: $("a"), then: [ {act: "msg", key: "msg1" }] })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(e.inputs.get::<String>(consts::ACT_VALUE).unwrap() == "u1");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_block() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.block({ then: [{ act: "msg", key: "msg1" }] })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("msg1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_call() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let dep = Workflow::new().with_id("m1");
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.call({ key: "m1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test_dep(&workflow, &dep, &pack, |e, s| {
        if e.is_key("m1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_push() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"act.push({ act: "irq", key: "act1" })"#.to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("act1") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_expose() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.expose("a", 100);
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<i32>("a").unwrap() == 100);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_abort() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.abort();
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("aborted") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_fail() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.fail("err1", "error message");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("error") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_skip() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.skip();
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("skipped") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_back() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.back("step1");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step2") && e.is_state("backed") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_state() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        let state = act.state();
        act.expose("state", state);
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<String>("state").unwrap() == "running");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_set_value() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_uses("pack1")
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        act.set("my_data", "abc");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(e.outputs.get::<String>("my_data").unwrap() == "abc");
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_throw_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_uses("pack1")
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        throw new Error("test error");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("error") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_step_uses_catch_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_output("my_data", json!(null))
            .with_uses("pack1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::msg(|act| act.with_key("msg1")))))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        data: br#"
        throw new Error("test error");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

async fn run_test<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    package: &data::Package,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    let s = rx.clone();
    engine.executor().pack().publish(package).unwrap();
    engine.channel().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });

    engine.channel().on_error(move |_| {
        s.close();
    });
    engine.runtime().launch(&proc);
    tx.recv().await
}

async fn run_test_dep<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    dep: &Workflow,
    package: &data::Package,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    engine.executor().model().deploy(dep).unwrap();
    engine.executor().pack().publish(package).unwrap();
    engine.channel().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });
    engine.runtime().launch(&proc);
    tx.recv().await
}
