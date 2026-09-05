use crate::{
    Variant, Workflow,
    utils::{
        self,
        test::{USES_CODE, auto_complete, create_proc},
    },
};
use serde_json::json;

use serial_test::serial;

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_code_get_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("abc", "test")
            .with_uses_code(
                USES_CODE,
                r#"
                let inputs = $inputs();
                inputs
            "#,
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_uses(USES_CODE)
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
        step.with_id("step1")
            .with_var("my_value", "abc")
            .with_uses_code(
                USES_CODE,
                r#"
                let data = $data();
                return { data: data.my_value }
            "#,
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_uses(USES_CODE)
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
        step.with_id("step1")
            .with_expose(Variant::create("my_output", json!(null)))
            .with_uses_code(
                USES_CODE,
                r#"
                return { "my_output": "abc" };
            "#,
            )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_uses(USES_CODE)
            .first()
            .unwrap()
            .outputs()
            .get::<String>("my_output")
            .unwrap(),
        "abc"
    );
}
