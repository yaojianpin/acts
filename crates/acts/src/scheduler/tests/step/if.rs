use crate::{
    TaskState, Workflow,
    utils::{self, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_if_false() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("false"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| {
            let sig = sig.clone();
            async move {
                sig.close();
            }
        }
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| {
            let sig = sig.clone();
            async move {
                sig.close();
            }
        }
    });

    rt.launch(&proc).await.unwrap();
    let _ = sig.recv().await;

    proc.print();

    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Skipped
    );

    assert_eq!(
        proc.task_by_nid("step2").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_if_true() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("true"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| {
            let sig = sig.clone();
            async move {
                sig.close();
            }
        }
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| {
            let sig = sig.clone();
            async move {
                sig.close();
            }
        }
    });

    rt.launch(&proc).await.unwrap();
    let _ = sig.recv().await;

    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("step2").first().unwrap().state(),
        TaskState::Completed
    );
}
