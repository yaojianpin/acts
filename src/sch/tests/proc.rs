use crate::{
    sch::{
        tests::{create_proc, create_proc_signal},
        NodeTree, TaskState,
    },
    utils, Workflow,
};

#[tokio::test]
async fn sch_proc_send() {
    let mut workflow = Workflow::default();
    let id = utils::longid();
    let (proc, scher, ..) = create_proc_signal::<()>(&mut workflow, &id);
    scher.launch(&proc);
    scher.next().await;

    assert!(scher.proc(&id).is_some())
}

#[tokio::test]
async fn sch_proc_state() {
    let mut workflow = Workflow::default();

    let id = utils::longid();
    let (proc, ..) = create_proc(&mut workflow, &id);

    proc.set_state(TaskState::Skip);
    assert_eq!(proc.state(), TaskState::Skip)
}

#[tokio::test]
async fn sch_proc_cost() {
    let mut workflow = Workflow::default();
    let id = utils::longid();
    let (proc, ..) = create_proc(&mut workflow, &id);

    proc.set_state(TaskState::Success);
    proc.set_start_time(100);
    proc.set_end_time(120);

    assert_eq!(proc.cost(), 20)
}

#[tokio::test]
async fn sch_proc_time() {
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let (proc, scher, .., tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;

    assert!(proc.start_time() > 0);
    assert!(proc.end_time() > 0)
}

#[tokio::test]
async fn sch_proc_task() {
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tr = NodeTree::build(&mut workflow).unwrap();
    let (proc, ..) = create_proc(&mut workflow, &pid);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);
    assert!(proc.task(&task.id).is_some())
}
