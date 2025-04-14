use serde_json::json;

use crate::{scheduler::tests::create_proc_signal, utils, Act, StmtBuild, Vars, Workflow};

#[tokio::test]
async fn sch_step_setup_expose_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", 5))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    let outputs = proc.task_by_nid("step1").first().unwrap().outputs();
    assert_eq!(outputs.get::<i64>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_step_setup_expose_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", 5).with("b", "bb"))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    let outputs = proc.task_by_nid("step1").first().unwrap().outputs();
    assert_eq!(outputs.get::<i64>("a").unwrap(), 5);
    assert_eq!(outputs.get::<String>("b").unwrap(), "bb");
}

#[tokio::test]
async fn sch_step_setup_expose_null() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", ()).with("b", ()))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    let outputs = proc.task_by_nid("step1").first().unwrap().outputs();
    assert!(outputs.get::<()>("a").is_some());
    assert!(outputs.get::<()>("b").is_some());
}

#[tokio::test]
async fn sch_step_setup_expose_local() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!("abc"))
            .with_input("b", json!(5))
            .with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", ()).with("b", ()))))
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    let outputs = proc.task_by_nid("step1").first().unwrap().outputs();
    assert_eq!(outputs.get::<String>("a"), Some("abc".to_owned()));
    assert_eq!(outputs.get::<i32>("b"), Some(5));
}

#[tokio::test]
async fn sch_step_setup_expose_update() {
    let mut workflow = Workflow::new()
        .with_input("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", "123"))))
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
            .get::<String>("a")
            .unwrap(),
        "123"
    );
}
