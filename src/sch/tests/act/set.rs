use crate::{sch::tests::create_proc_signal, utils, Act, StmtBuild, Vars, Workflow};
use serde_json::json;

#[tokio::test]
async fn sch_act_set_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", 5))))
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("a")
            .unwrap(),
        5
    );

    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
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
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"${ $("b") }"#))))
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
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"${ $("a") + "bc" }"#))))
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
            .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"${ $("a") + 20 }"#))))
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
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
            step.with_id("step1")
                .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", r#"${ $("b") }"#))))
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
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.data().get::<String>("b").unwrap(), "123");
}
