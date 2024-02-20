use crate::{sch::tests::create_proc, utils, Act, Action, Vars, Workflow};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_vars_workflow_inputs() {
    let mut workflow = Workflow::new().with_input("var1", 10.into());
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().root().get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_workflow_outputs_value() {
    let outputs = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new().with_output("var1", 10.into());
    let (proc, scher, emiter) = create_proc(&mut workflow, &utils::longid());

    let o = outputs.clone();
    emiter.on_complete(move |e| {
        *o.lock().unwrap() = e.outputs().clone();
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(outputs.lock().unwrap().get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_workflow_outputs_script() {
    let outputs = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new()
        .with_input("a", json!(10))
        .with_output("var1", json!(r#"${ env.get("a") }"#));
    let (proc, scher, emiter) = create_proc(&mut workflow, &utils::longid());

    let o = outputs.clone();
    emiter.on_complete(move |e| {
        *o.lock().unwrap() = e.outputs().clone();
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(outputs.lock().unwrap().get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_get_with_script() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_input("var2", r#"${ env.get("var1") }"#.into())
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .inputs()
            .get_value("var2")
            .unwrap(),
        &json!(10)
    );
}

#[tokio::test]
async fn sch_vars_get_with_not_exists() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var2", r#"${ env.get("var1") }"#.into())
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .inputs()
            .get_value("var2")
            .unwrap(),
        &json!(null)
    );
}

#[tokio::test]
async fn sch_vars_output_only_key_name() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_output("var1", json!(null))
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .outputs()
            .get_value("var1")
            .unwrap(),
        &json!(10)
    );
}

#[tokio::test]
async fn sch_vars_step_inputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_input("var1", 10.into()));
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_one_step_outputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_output("var1", 10.into()));

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().root().get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_two_steps_outputs() {
    let mut workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_output("var1", 10.into()))
        .with_step(|step| step.with_id("step2").with_output("var1", 20.into()));

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<i64>("var1").unwrap(), 20);
}

#[tokio::test]
async fn sch_vars_branch_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_input("var1", 10.into()))
    });

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("b1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_branch_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_output("var1", 10.into())
        })
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_branch_one_step_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_input("var1", json!(10))
                .with_step(|step| step.with_id("step1").with_output("var1", json!(100)))
        })
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        100
    );
}

#[tokio::test]
async fn sch_vars_branch_two_steps_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_input("var1", json!(10))
                .with_step(|step| step.with_id("step1").with_output("var1", json!(100)))
                .with_step(|step| step.with_id("step2").with_output("var1", json!(200)))
        })
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        200
    );
}

#[tokio::test]
async fn sch_vars_act_inputs() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1").with_input("var1", 10)))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state("created") {
            *r.lock().unwrap() = e.inner().inputs.get_value("var1").unwrap() == &json!(10);
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_vars_act_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(Act::req(|act| {
            act.with_id("act1").with_output("var1", json!(null))
        }))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            s.do_action(&action).unwrap();
        }
    });

    emitter.on_complete(|e| e.close());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_act_options() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.inner().is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            s.do_action(&action).unwrap();
        }
    });

    emitter.on_complete(|e| e.close());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("act1")
            .get(0)
            .unwrap()
            .env()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_get_global_vars() {
    let mut workflow = Workflow::new()
        .with_input("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1").with_act(Act::req(|act| {
                act.with_id("act1").with_output("var1", json!(null))
            }))
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.inner().is_source("act") && e.inner().is_state("created") {
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get_env::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn sch_vars_override_global_vars() {
    let mut workflow = Workflow::new()
        .with_input("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1").with_act(Act::req(|act| {
                act.with_id("act1").with_output("var1", json!(null))
            }))
        });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state("created") {
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    proc.task_by_nid("step1")
        .get(0)
        .unwrap()
        .env()
        .set_env(&Vars::new().with("a", 10));
    assert_eq!(proc.env().root().get::<i32>("a").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_override_step_vars() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!("abc"))
            .with_id("step1")
            .with_act(Act::req(|act| {
                act.with_id("act1").with_output("var1", json!(null))
            }))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state("created") {
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    proc.task_by_nid("act1")
        .get(0)
        .unwrap()
        .env()
        .set_env(&Vars::new().with("a", 10));
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<i32>("a")
            .unwrap(),
        json!(10)
    );
}
