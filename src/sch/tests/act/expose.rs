use serde_json::json;

use crate::{sch::tests::create_proc, utils, Act, StmtBuild, Vars, Workflow};

#[tokio::test]
async fn sch_step_setup_expose_one() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", 5))))
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<i64>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_step_setup_expose_many() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", 5).with("b", "bb"))))
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<i64>("a").unwrap(), 5);

    assert_eq!(proc.env().root().get::<String>("b").unwrap(), "bb");
}

#[tokio::test]
async fn sch_step_setup_expose_null() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", ()).with("b", ()))))
    });

    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<()>("a").unwrap(), ());

    assert_eq!(proc.env().root().get::<()>("b").unwrap(), ());
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
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<String>("a").unwrap(), "abc");
    assert_eq!(proc.env().root().get::<i32>("b").unwrap(), 5);
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
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.env().root().get::<String>("a").unwrap(), "123");
}
