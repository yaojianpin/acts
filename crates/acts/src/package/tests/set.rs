use crate::{
    Act, Vars, Workflow,
    utils::{
        self,
        test::{USES_SET, auto_complete, create_proc},
    },
};
use serde_json::json;

use serial_test::serial;
#[test]
fn pack_set_parse_primary() {
    let text = r#"
    uses: acts.transform.set
    params:
        a: 1
        b: abc
    "#;

    let act = serde_yaml::from_str::<Act>(text).unwrap();
    assert_eq!(act.uses, "acts.transform.set");

    let params: Vars = act.params.into();
    assert_eq!(params.get::<i32>("a").unwrap(), 1);
    assert_eq!(params.get::<String>("b").unwrap(), "abc");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_one() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_SET, Vars::new().with("a", 5))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_many() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_SET, Vars::new().with("a", 5).with("b", "bb"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_local_var() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("b", json!("abc"))
            .with_uses(USES_SET, Vars::new().with("a", "${{ b }}"))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_calc_str() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!("a"))
            .with_uses(USES_SET, Vars::new().with("a", r#"${{ a + "bc" }}"#))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_calc_int() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(10))
            .with_uses(USES_SET, Vars::new().with("a", r#"${{ a + 20 }}"#))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {e:?}");
    });
    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_update_local() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("b", json!("abc"))
            .with_uses(USES_SET, Vars::new().with("a", r#"123"#))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_get_global_var() {
    let workflow = Workflow::new()
        .with_var("b", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_SET, Vars::new().with("a", r#"${{ b }}"#))
        });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_set_global_var() {
    let workflow = Workflow::new()
        .with_var("b", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_SET, Vars::new().with("b", r#"123"#))
        });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(proc.data().get::<String>("b").unwrap(), "123");
}
