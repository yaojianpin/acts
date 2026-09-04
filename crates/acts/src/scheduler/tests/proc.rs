use crate::{Config, config::ConfigData};
use crate::{
    Workflow,
    scheduler::{NodeTree, TaskState},
    utils::{
        self,
        test::{USES_CODE, auto_complete, create_proc, create_proc_with_config},
    },
};
use serial_test::serial;

#[tokio::test]
async fn sch_proc_send() {
    let workflow = Workflow::default().with_step(|step| step.with_id("step1"));
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    auto_complete(&engine, &sig);
    rt.launch(&proc).unwrap();
    rt.queue().next().await.unwrap();

    assert!(rt.proc(&id).unwrap().is_some())
}

#[tokio::test]
async fn sch_proc_state() {
    let workflow = Workflow::default();

    let id = utils::longid();
    let (_, proc) = create_proc(&workflow, &id);

    proc.set_state(TaskState::Skipped);
    assert_eq!(proc.state(), TaskState::Skipped)
}

#[tokio::test]
async fn sch_proc_cost() {
    let workflow = Workflow::default();
    let id = utils::longid();
    let (_, proc) = create_proc(&workflow, &id);

    proc.set_state(TaskState::Completed);
    proc.set_start_time(100);
    proc.set_end_time(120);

    assert_eq!(proc.cost(), 20)
}

#[tokio::test]
async fn sch_proc_time() {
    let workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let tx = engine.signal(());
    auto_complete(&engine, &tx);
    rt.launch(&proc).unwrap();
    tx.recv().await;

    assert!(proc.start_time() > 0);
    assert!(proc.end_time() > 0)
}

#[tokio::test]
async fn sch_proc_task() {
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tr = NodeTree::build(&mut workflow).unwrap();
    let (_, proc) = create_proc(&workflow, &pid);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None).unwrap();
    assert!(proc.task(&task.id).is_some())
}

#[tokio::test]
async fn sch_proc_start_single_shot() {
    // `start()` is an atomic state machine: the first call takes the process
    // from `None` to `Running` and schedules the root task; any repeated call
    // — while running or after completion — is a no-op and must not replace
    // the root task, reset the start time, or re-execute the workflow (which
    // would duplicate tasks and spawn extra per-process tick loops).
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("s1"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let sig = engine.signal(());
    auto_complete(&engine, &sig);

    engine.runtime().launch(&proc).unwrap();
    let start_time = proc.start_time();
    assert!(start_time > 0, "start must set the start time");

    // repeated start while the process is running (or already finished) is a
    // no-op — the start time must not be reset
    proc.start().unwrap();
    assert_eq!(
        proc.start_time(),
        start_time,
        "a repeated start must not reset the start time"
    );

    sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(
        proc.task_by_nid("s1").len(),
        1,
        "a repeated start must not re-execute the workflow root"
    );

    // even after completion a repeated start must not restart the process
    proc.start().unwrap();
    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(proc.task_by_nid("s1").len(), 1);
    assert_eq!(proc.start_time(), start_time);
    drop(engine);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_proc_node_self_loop_bounded() {
    // A step whose `next` points back to itself would otherwise create a new
    // task on every loop iteration forever; the per-node run limit
    // (`max_node_run_times`) must abort the process instead.
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("s1").with_next("s1"));

    let config = Config {
        data: ConfigData {
            max_node_run_times: Some(3),
            ..Default::default()
        },
        table: Default::default(),
    };
    let (engine, proc) = create_proc_with_config(&config, &workflow, &utils::longid());
    let sig = engine.signal(());
    auto_complete(&engine, &sig);
    engine.runtime().launch(&proc).unwrap();
    sig.recv().await;

    // the process ended in error, with exactly `max_node_run_times` task
    // instances created for the node (the next scheduling was rejected)
    assert_eq!(proc.state(), TaskState::Error);
    assert_eq!(proc.task_by_nid("s1").len(), 3);
    assert_eq!(proc.tasks().len(), 4); // root + 3 x s1
}
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_proc_do_tick_skips_terminal_states() {
    // `do_tick` must only fire timeouts for tasks still in flight: a
    // completed / error / aborted / skipped step task must not schedule its
    // timeout children on any tick
    let workflow = Workflow::new().with_id("w1").with_step(|step| {
        step.with_id("s1")
            .with_timeout(|timeout| timeout.with_id("t1"))
    });

    for state in [
        TaskState::Completed,
        TaskState::Error,
        TaskState::Aborted,
        TaskState::Skipped,
    ] {
        let (engine, proc) = create_proc(&workflow, &utils::longid());
        let s1 = proc.tree().node("s1").unwrap();
        let task = proc.create_task(&s1, None).unwrap();
        task.set_state(state.clone());
        proc.do_tick();
        assert!(
            proc.task_by_nid("t1").is_empty(),
            "a {state} task must not fire its timeouts"
        );
        drop(engine);
    }

    // control: a running task still fires its timeouts
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let s1 = proc.tree().node("s1").unwrap();
    let task = proc.create_task(&s1, None).unwrap();
    task.set_state(TaskState::Running);
    proc.do_tick();
    assert_eq!(proc.task_by_nid("t1").len(), 1);
    drop(engine);
}

/// A step whose `if` guard holds and whose `next` points back at itself is a
/// bounded while loop: it keeps re-executing while the condition holds, then —
/// once the condition fails and the step is skipped — falls through to the
/// step declared after it.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_while_loop_self_next() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_var("index", 0)
        .with_step(|step| {
            step.with_id("add")
                .with_if(r#"index < 3"#)
                .with_next("add")
                .with_uses_code(USES_CODE, r#"$set("index", $get("index") + 1);"#)
        })
        .with_step(|step| step.with_id("end"));

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let sig = engine.signal(());
    auto_complete(&engine, &sig);
    engine.runtime().launch(&proc).unwrap();
    sig.recv().await;

    assert_eq!(proc.state(), TaskState::Completed);
    // index 0,1,2 run the body; the index=3 evaluation is skipped and exits
    let add_tasks = proc.task_by_nid("add");
    assert_eq!(add_tasks.len(), 4);
    assert_eq!(
        add_tasks.iter().filter(|t| t.state().is_success()).count(),
        3
    );
    assert!(add_tasks.last().unwrap().state().is_skip());
    assert_eq!(proc.task_by_nid("end").len(), 1);
    assert!(
        proc.task_by_nid("end")
            .first()
            .unwrap()
            .state()
            .is_completed()
    );
    drop(engine);
}

/// A step with a `while` condition is a bounded loop: the body re-executes
/// while the condition holds, then the step is skipped and the flow falls
/// through to the next declared step. No self-`next` is needed.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_step_while_attr_loops() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_var("index", 0)
        .with_step(|step| {
            step.with_id("add")
                .with_while(r#"index < 3"#)
                .with_uses_code(USES_CODE, r#"$set("index", $get("index") + 1);"#)
        })
        .with_step(|step| step.with_id("end"));

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let sig = engine.signal(());
    auto_complete(&engine, &sig);
    engine.runtime().launch(&proc).unwrap();
    sig.recv().await;

    assert_eq!(proc.state(), TaskState::Completed);
    let add_tasks = proc.task_by_nid("add");
    assert_eq!(add_tasks.len(), 4);
    assert_eq!(
        add_tasks.iter().filter(|t| t.state().is_success()).count(),
        3
    );
    assert!(add_tasks.last().unwrap().state().is_skip());
    assert_eq!(proc.task_by_nid("end").len(), 1);
    assert!(
        proc.task_by_nid("end")
            .first()
            .unwrap()
            .state()
            .is_completed()
    );
    drop(engine);
}

/// A step skipped by its `if` condition must not take its explicit `next`
/// (a self/backward `next` would otherwise re-schedule it forever): it falls
/// through to the following step instead.
#[serial]
#[tokio::test(flavor = "multi_thread")]

async fn sch_step_if_false_self_next_falls_through() {
    let workflow = Workflow::new()
        .with_id("w1")
        .with_step(|step| step.with_id("s1").with_if("false").with_next("s1"))
        .with_step(|step| step.with_id("s2"));

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let sig = engine.signal(());
    auto_complete(&engine, &sig);
    engine.runtime().launch(&proc).unwrap();
    sig.recv().await;

    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(proc.task_by_nid("s1").len(), 1);
    assert!(proc.task_by_nid("s1").first().unwrap().state().is_skip());
    assert_eq!(proc.task_by_nid("s2").len(), 1);
    assert!(
        proc.task_by_nid("s2")
            .first()
            .unwrap()
            .state()
            .is_completed()
    );
    drop(engine);
}
