use crate::{
    MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    utils::{self, test::USES_IRQ, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_state() {
    let workflow = Workflow::new();
    let (_engine, proc) = create_proc(&workflow, "w1");
    assert!(proc.state() == TaskState::None);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_start() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, "w1");
    let rt = engine.runtime();
    let sig = engine.signal(TaskState::default());
    let tx = sig.clone();
    let rx = sig.clone();

    proc.start().unwrap();
    rt.emitter().on_proc(move |e| rx.send(e.state()));

    let ret = tx.recv().await;
    assert_eq!(ret, TaskState::Running);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_steps() {
    let workflow = Workflow::new()
        .with_step(|mut step| {
            step.name = "step1".to_string();
            step
        })
        .with_step(|mut step| {
            step.name = "step2".to_string();
            step
        });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_completed() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(false);
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    emitter.on_message(move |msg| {
        if msg.inner().r#type == "step" && msg.inner().state() == MessageState::Completed {
            rx.send(true);
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret)
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_skip_with_inputs_to_next() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_var("v1", 10)
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::new());
    let rx = sig.clone();
    let rt2 = rt.clone();
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
            && e.is_state(MessageState::Created)
        {
            rt2.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                .unwrap();
        }

        if e.params().unwrap().get::<String>("key").as_deref() == Some("act2")
            && e.is_state(MessageState::Created)
        {
            rx.send(e.inputs.clone());
        }
    });

    rt.launch(&proc).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret.get::<i32>("v1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_empty_acts() {
    let workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let id = utils::longid();
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_if_false() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("false"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
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
async fn sch_task_step_if_true() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("true"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_basic() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_if("true")
                    .with_name("branch 1")
                    .with_step(|step| step.with_name("step11"))
                    .with_step(|step| step.with_name("step12"))
                    .with_step(|step| step.with_name("step13"))
            })
            .with_branch(|branch| {
                branch
                    .with_name("branch 2")
                    .with_step(|step| step.with_name("step21"))
            })
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_skip() {
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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });
    // process.tree().print();

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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });
    // process.tree().print();

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
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });
    // process.tree().print();

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

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_act_skip_with_inputs_to_next() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("v1", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::new());
    let rx = sig.clone();
    let rt2 = rt.clone();
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
            && e.is_state(MessageState::Created)
        {
            rt2.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                .unwrap();
        }

        if e.params().unwrap().get::<String>("key").as_deref() == Some("act2")
            && e.is_state(MessageState::Created)
        {
            rx.send(e.inputs.clone());
        }
    });

    rt.launch(&proc).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret.get::<i32>("v1").unwrap(), 10);
}
