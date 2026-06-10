use crate::{
    TaskState, Vars, Workflow,
    utils::{self, test::USES_IRQ, test::auto_complete, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_branch_skip() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_branch(|branch| {
            branch
                .with_id("b1")
                .with_if("false")
                .with_name("branch 1")
                .with_step(|step| step.with_id("step11"))
                .with_step(|step| step.with_id("step12"))
                .with_step(|step| step.with_id("step13"))
        })
    });

    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);

    let rt = engine.runtime();
    let (sig, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;

    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Skipped
    );
    assert!(proc.task_by_nid("step11").is_empty());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_needs_state() {
    let workflow = Workflow::new().with_var("v", 5).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"v > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| {
                        step.with_id("step11")
                            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                    })
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"v > 2"#)
                    .with_name("branch 2")
                    .with_need("b1")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    workflow.print();
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    let rx = sig.clone();
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.inner().is_type("act") {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Running
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Pending
    );
}
