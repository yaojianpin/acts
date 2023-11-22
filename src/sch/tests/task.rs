use crate::{
    event::{ActionState, Emitter},
    sch::{Proc, Scheduler},
    utils, Engine, TaskState, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_task_state() {
    let mut workflow = Workflow::new();
    let (proc, _, _) = create_proc(&mut workflow, "w1");
    assert!(proc.state() == TaskState::None);
}

#[tokio::test]
async fn sch_task_start() {
    let mut workflow = Workflow::new();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    proc.start(&scher);
    emitter.on_proc(|proc| {
        assert_eq!(proc.state(), TaskState::Running);
    });
}

#[tokio::test]
async fn sch_task_steps() {
    let mut workflow = Workflow::new()
        .with_step(|mut step| {
            step.name = "step1".to_string();
            step
        })
        .with_step(|mut step| {
            step.name = "step2".to_string();
            step
        });
    let id = utils::longid();
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_task_step_completed() {
    let mut workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(|msg| {
        if msg.inner().r#type == "step" && msg.inner().state() == ActionState::Completed {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_task_step() {
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let id = utils::longid();
    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_task_step_if_false() {
    let mut workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("false"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;

    proc.print();

    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Skip
    );

    assert_eq!(
        proc.task_by_nid("step2").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_task_step_if_true() {
    let mut workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_if("true"))
        .with_step(|step| step.with_id("step2"));
    workflow.print();
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;

    proc.print();
    assert_eq!(
        proc.task_by_nid("step1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("step2").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test]
async fn sch_task_branch_basic() {
    let mut workflow = Workflow::new().with_step(|step| {
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

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_task_branch_skip() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Skip
    );
    assert_eq!(proc.task_by_nid("step11").get(0).is_none(), true);
}

#[tokio::test]
async fn sch_task_branch_empty_if() {
    let mut workflow = Workflow::new().with_step(|step| {
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
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Skip
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_false_else_success() {
    let mut workflow = Workflow::new().with_env("v", 1.into()).with_step(|step| {
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
                    .with_if(r#"env.get("v") < 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Success
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_false_else_running() {
    let mut workflow = Workflow::new().with_env("v", 1.into()).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_else(true)
                    .with_name("branch 1")
                    .with_step(|step| step.with_name("step11").with_act(|act| act.with_id("act1")))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"env.get("v") < 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    // proc.tree().print();
    emitter.on_message(|e| {
        if e.is_key("act1") {
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Running
    );

    // check the branch state is updated to store
    let task = proc.task_by_nid("b1").get(0).unwrap().clone();
    let task_id = utils::Id::new(&task.proc_id, &task.id);
    assert_eq!(
        scher
            .cache()
            .store()
            .base()
            .tasks()
            .find(&task_id.id())
            .unwrap()
            .state,
        "running"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_true_else() {
    let mut workflow = Workflow::new().with_env("v", 1.into()).with_step(|step| {
        step.with_id("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"env.get("v") > 0"#)
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
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("b2").get(0).unwrap().state(),
        TaskState::Skip
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_two_no_else() {
    let mut workflow = Workflow::new().with_env("v", 1.into()).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"env.get("v") > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"env.get("v") <= 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("b2").get(0).unwrap().state(),
        TaskState::Skip
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_if_mutli_true() {
    let mut workflow = Workflow::new().with_env("v", 5.into()).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"env.get("v") > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"env.get("v") <= 0"#)
                    .with_name("branch 2")
                    .with_step(|step| step.with_id("step21"))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b3")
                    .with_if(r#"env.get("v") > 2"#)
                    .with_name("branch 3")
                    .with_step(|step| step.with_id("step31"))
            })
    });

    let id = utils::longid();
    let (proc, scher, _) = create_proc(&mut workflow, &id);
    // proc.tree().print();

    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("b3").get(0).unwrap().state(),
        TaskState::Success
    );
    assert_eq!(
        proc.task_by_nid("b2").get(0).unwrap().state(),
        TaskState::Skip
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_needs_state() {
    let mut workflow = Workflow::new().with_env("v", 5.into()).with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_id("b1")
                    .with_if(r#"env.get("v") > 0"#)
                    .with_name("branch 1")
                    .with_step(|step| step.with_id("step11").with_act(|act| act.with_id("act1")))
            })
            .with_branch(|branch| {
                branch
                    .with_id("b2")
                    .with_if(r#"env.get("v") > 2"#)
                    .with_name("branch 2")
                    .with_need("b1")
                    .with_step(|step| step.with_id("step21"))
            })
    });

    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("b1").get(0).unwrap().state(),
        TaskState::Running
    );
    assert_eq!(
        proc.task_by_nid("b2").get(0).unwrap().state(),
        TaskState::Pending
    );
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let s = scher.clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            s.close();
        }
    });

    let s2 = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s2.close();
    });
    (proc, scher, emitter)
}
