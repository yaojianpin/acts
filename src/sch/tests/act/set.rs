use serde_json::json;

use crate::{
    event::Emitter,
    sch::{Proc, Scheduler},
    utils, Act, Engine, StmtBuild, Vars, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_act_set_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", 5))))
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
            .env()
            .get::<i64>("a")
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn sch_act_set_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", 5).with("b", "bb"))))
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
            .env()
            .get::<i64>("a")
            .unwrap(),
        5
    );

    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .env()
            .get::<String>("b")
            .unwrap(),
        "bb"
    );
}

#[tokio::test]
async fn sch_act_set_local_var() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("b", json!("abc"))
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"${ env.get("b") }"#))))
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
            .env()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn sch_act_set_calc_str() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!("a"))
            .with_setup(|setup| {
                setup.add(Act::set(
                    Vars::new().with("a", r#"${ env.get("a") + "bc" }"#),
                ))
            })
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
            .env()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn sch_act_set_calc_int() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!(10))
            .with_setup(|setup| {
                setup.add(Act::set(
                    Vars::new().with("a", r#"${ env.get("a") + 20 }"#),
                ))
            })
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
            .env()
            .get::<i32>("a")
            .unwrap(),
        30
    );
}

#[tokio::test]
async fn sch_act_set_update_local() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("b", json!("abc"))
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"123"#))))
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
            .env()
            .get::<String>("a")
            .unwrap(),
        "123"
    );
}

#[tokio::test]
async fn sch_act_get_global_var() {
    let mut workflow = Workflow::new()
        .with_input("b", json!("abc"))
        .with_step(|step| {
            step.with_id("step1").with_setup(|setup| {
                setup.add(Act::set(Vars::new().with("a", r#"${ env.get("b") }"#)))
            })
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
            .env()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn sch_act_set_global_var() {
    let mut workflow = Workflow::new()
        .with_input("b", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_setup(|setup| setup.add(Act::set(Vars::new().with("b", r#"123"#))))
        });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<String>("b").unwrap(), "123");
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            p.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        p.close();
    });
    (proc, scher, emitter)
}
