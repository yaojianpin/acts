use serde_json::json;

use crate::{
    Act, Workflow,
    utils::{
        self,
        test::{auto_complete, create_proc},
    },
};

use serial_test::serial;

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_code_get_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                let inputs = $inputs();
                inputs
            "#,
            )
            .with_id("code1")
            .with_var("abc", "test"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("code1")
            .first()
            .unwrap()
            .inputs()
            .get::<String>("abc")
            .unwrap(),
        "test"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_code_get_data() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                let data = $data();
                return { data: data.my_value }
            "#,
            )
            .with_id("code1")
            .with_var("my_value", "abc"),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("code1")
            .first()
            .unwrap()
            .outputs()
            .get::<String>("data")
            .unwrap(),
        "abc"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_code_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                return { "my_output": "abc" };
            "#,
            )
            .with_id("code1")
            .with_expose("my_output", json!(null)),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("code1")
            .first()
            .unwrap()
            .outputs()
            .get::<String>("my_output")
            .unwrap(),
        "abc"
    );
}
