use serde_json::json;

use crate::{
    Act, Workflow,
    utils::{self, consts, test::create_proc_signal},
};

#[tokio::test]
async fn pack_code_get_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                let inputs = $act.inputs();
                inputs
            "#,
            )
            .with_id("code1")
            .with_input("abc", "test"),
        )
    });

    workflow.print();
    let (proc, scher, _emitter, tx, _rx) =
        create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
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

#[tokio::test]
async fn pack_code_get_data() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                let data = $act.data();
                return { data: data.my_value }
            "#,
            )
            .with_id("code1")
            .with_input("my_value", "abc"),
        )
    });

    workflow.print();
    let (proc, scher, _emitter, tx, _rx) =
        create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("code1")
            .first()
            .unwrap()
            .outputs()
            .get::<String>(consts::ACT_DATA)
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn pack_code_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::code(
                r#"
                return { "my_output": "abc" };
            "#,
            )
            .with_id("code1")
            .with_output("my_output", json!(null)),
        )
    });

    workflow.print();
    let (proc, scher, _, tx, _rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
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
