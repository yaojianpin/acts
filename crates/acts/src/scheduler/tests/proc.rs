use crate::{
    Workflow,
    scheduler::{NodeTree, TaskState},
    utils::{
        self,
        test::{auto_complete, create_proc},
    },
};

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
    let mut workflow = Workflow::default();

    let id = utils::longid();
    let (_, proc) = create_proc(&mut workflow, &id);

    proc.set_state(TaskState::Skipped);
    assert_eq!(proc.state(), TaskState::Skipped)
}

#[tokio::test]
async fn sch_proc_cost() {
    let mut workflow = Workflow::default();
    let id = utils::longid();
    let (_, proc) = create_proc(&mut workflow, &id);

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
    let (_, proc) = create_proc(&mut workflow, &pid);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);
    assert!(proc.task(&task.id).is_some())
}
