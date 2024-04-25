use crate::{
    data,
    sch::{tests::create_proc_signal2, Proc},
    utils::{self, consts},
    Act, Event, Message, Signal, StmtBuild, TaskState, Workflow,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sch_act_pack_msg() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.msg({ key: "msg1" })"#.to_vec(),
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
async fn sch_act_pack_req() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_uses("pack1"));
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.req({ key: "act1" })"#.to_vec(),
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
async fn sch_act_pack_chain_array() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.chain({ in: '["u1"]', run: [ { msg: { key: "msg1" } } ] })"#.to_vec(),
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
async fn sch_act_pack_chain_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::pack(|p| p.with_uses("pack1")))
        });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.chain({ in: $("a"), run: [ { msg: { key: "msg1" } } ] })"#.to_vec(),
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
async fn sch_act_pack_each_array() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.each({ in: '["u1"]', run: [ { msg: { key: "msg1" } } ] })"#.to_vec(),
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
async fn sch_act_pack_each_var() {
    let workflow = Workflow::new()
        .with_input("a", json!(r#"["u1"]"#))
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::pack(|p| p.with_uses("pack1")))
        });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.each({ in: $("a"), run: [ { msg: { key: "msg1" } } ] })"#.to_vec(),
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
async fn sch_act_pack_block() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.block({ acts: [{ msg: { id: "msg1" } }] })"#.to_vec(),
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
async fn sch_act_pack_call() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let dep = Workflow::new().with_id("m1");
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.call({ mid: "m1" })"#.to_vec(),
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
async fn sch_act_pack_push() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"act.push({ req: { id: "act1" } })"#.to_vec(),
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
async fn sch_act_pack_set_output() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_id("pack1").with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        act.set_output("a", 100);
        "#
        .to_vec(),
        ..Default::default()
    };
    let (_, proc) = run_test_proc::<()>(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.close();
        }
    })
    .await;

    assert_eq!(
        proc.task_by_nid("pack1")
            .get(0)
            .unwrap()
            .outputs()
            .get::<i32>("a")
            .unwrap(),
        100
    );
}

#[tokio::test]
async fn sch_act_pack_abort() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
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
async fn sch_act_pack_fail() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
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
async fn sch_act_pack_fail_catch() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| {
                c.with_err("err1")
                    .with_then(|stmts| stmts.add(Act::msg(|act| act.with_id("msg1"))))
            })
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        act.fail("err1", "error message");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.send(true);
        }
    })
    .await;
    assert!(ret);
}

#[tokio::test]
async fn sch_act_pack_skip() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_id("pack1").with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        act.skip();
        "#
        .to_vec(),
        ..Default::default()
    };
    let (_, proc) = run_test_proc::<()>(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.close();
        }
    })
    .await;
    assert_eq!(
        proc.task_by_nid("pack1").get(0).unwrap().state(),
        TaskState::Skip
    );
}

#[tokio::test]
async fn sch_act_pack_back() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::pack(|p| p.with_uses("pack1")))
        });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
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
async fn sch_act_pack_state() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_id("pack1").with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        let state = act.state();
        act.set_output("state", state);
        "#
        .to_vec(),
        ..Default::default()
    };
    let (_, proc) = run_test_proc::<()>(&workflow, &pack, |e, s| {
        if e.is_key("step1") && e.is_state("completed") {
            s.close();
        }
    })
    .await;
    assert_eq!(
        proc.task_by_nid("pack1")
            .get(0)
            .unwrap()
            .outputs()
            .get::<String>("state")
            .unwrap(),
        "running"
    );
}

#[tokio::test]
async fn sch_act_pack_throw_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        throw new Error("test error");
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
async fn sch_act_pack_catch_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|c| c.with_then(|stmts| stmts.add(Act::msg(|act| act.with_id("msg1")))))
            .with_act(Act::pack(|p| p.with_uses("pack1")))
    });
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: br#"
        throw new Error("test error");
        "#
        .to_vec(),
        ..Default::default()
    };
    let ret: bool = run_test(&workflow, &pack, |e, s| {
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
    engine.manager().publish(&package).unwrap();
    engine.emitter().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });
    engine.scher().launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    ret
}

async fn run_test_proc<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    package: &data::Package,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> (T, Arc<Proc>) {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    engine.manager().publish(&package).unwrap();
    engine.emitter().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });
    engine.scher().launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    (ret, proc.clone())
}

async fn run_test_dep<T: Clone + Send + 'static + Default>(
    workflow: &Workflow,
    dep: &Workflow,
    package: &data::Package,
    exit_if: fn(&Event<Message>, sig: Signal<T>),
) -> T {
    let (engine, proc, tx, rx) = create_proc_signal2::<T>(workflow, &utils::longid());
    engine.manager().deploy(&dep).unwrap();
    engine.manager().publish(&package).unwrap();
    engine.emitter().on_message(move |e| {
        println!("message: {:?}", e);
        exit_if(e, rx.clone());
    });
    engine.scher().launch(&proc);
    tx.recv().await
}
