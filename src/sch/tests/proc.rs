use crate::{
    sch::{NodeTree, Proc, Scheduler, TaskState},
    utils, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_proc_send() {
    let mut workflow = Workflow::default();
    let id = utils::longid();
    let (proc, scher) = create_proc(&mut workflow, &id);
    scher.launch(&proc);
    scher.next().await;

    assert!(scher.proc(&id).is_some())
}

#[tokio::test]
async fn sch_proc_state() {
    let mut workflow = Workflow::default();

    let id = utils::longid();
    let (proc, _) = create_proc(&mut workflow, &id);

    proc.set_state(TaskState::Skip);
    assert_eq!(proc.state(), TaskState::Skip)
}

#[tokio::test]
async fn sch_proc_cost() {
    let mut workflow = Workflow::default();
    let id = utils::longid();
    let (proc, _) = create_proc(&mut workflow, &id);

    proc.set_state(TaskState::Success);
    proc.set_start_time(100);
    proc.set_end_time(120);

    assert_eq!(proc.cost(), 20)
}

#[tokio::test]
async fn sch_proc_time() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1")
            .with_step(|step| step.with_name("step1"))
    });
    let (proc, scher) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;

    assert!(proc.start_time() > 0);
    assert!(proc.end_time() > 0)
}

#[tokio::test]
async fn sch_proc_task() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1")
            .with_step(|step| step.with_name("step1"))
    });

    let pid = utils::longid();
    let tr = NodeTree::build(&mut workflow);
    let (proc, _) = create_proc(&mut workflow, &pid);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);
    assert!(proc.task(&task.id).is_some())
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Scheduler>) {
    let scher = Scheduler::new();
    let proc = scher.create_proc(id, workflow);

    let evt = scher.emitter();
    let s = scher.clone();
    evt.on_complete(move |p| {
        if p.inner().state.is_completed() {
            s.close();
        }
    });

    let s2 = scher.clone();
    evt.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s2.close();
    });

    (proc, scher)
}
