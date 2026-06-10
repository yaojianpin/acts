use crate::{
    TaskState, Vars, Workflow,
    utils::{self, test::USES_IRQ, test::auto_complete, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_empty_if() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1").with_branch(|branch| {
            branch
                .with_id("b1")
                .with_name("branch 1")
                .with_step(|step| step.with_name("step11"))
                .with_step(|step| step.with_name("step12"))
                .with_step(|step| step.with_name("step13"))
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
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_false_else_success() {
    let workflow = Workflow::new().with_var("v", 1).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_else(true)
                    .with_name("branch 1")
                    .with_step(|step| step.with_name("step11"))
                    .with_step(|step| step.with_name("step12"))
                    .with_step(|step| step.with_name("step13"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"v < 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let (sig, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Completed
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_false_else_running() {
    let workflow = Workflow::new().with_var("v", 1).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_else(true)
                    .with_name("branch 1")
                    .with_step(|step| {
                        step.with_name("step11")
                            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
                    })
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"v < 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    let rx = sig.clone();
    let emitter = engine.channel();
    // process.tree().print();
    emitter.on_message(move |e| {
        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1") {
            rx.close();
        }
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;

    assert_eq!(
        proc.task_by_nid("b1").first().unwrap().state(),
        TaskState::Running
    );

    // check the branch state is updated to store
    let task = proc.task_by_nid("b1").first().unwrap().clone();
    let task_id = utils::Id::new(&task.pid, &task.id);
    assert_eq!(
        rt.cache()
            .store()
            .tasks()
            .find(&task_id.id())
            .unwrap()
            .state,
        "running"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_true_else() {
    let workflow = Workflow::new().with_var("v", 1).with_step(|step| {
        step.with_id("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"v > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_else(true)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
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
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_two_no_else() {
    let workflow = Workflow::new().with_var("v", 1).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"v > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"v <= 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
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
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Skipped
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_mutli_true() {
    let workflow = Workflow::new().with_var("v", 5).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"v > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"v <= 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b3")
                    .with_if(r#"v > 2"#)
                    .with_name("branch 3")
                    .with_step(|step| step.with_id("step31"))
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
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("b3").first().unwrap().state(),
        TaskState::Completed
    );
    assert_eq!(
        proc.task_by_nid("b2").first().unwrap().state(),
        TaskState::Skipped
    );
}
