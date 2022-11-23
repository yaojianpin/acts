use crate::{
    sch::{event::EventData, Proc, Scheduler},
    TaskState, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn task_state() {
    let mut workflow = create_workflow();
    let (task, _) = create_proc(&mut workflow, "job1");
    assert!(task.state() == TaskState::None);
}

#[tokio::test]
async fn task_start() {
    let mut workflow = create_workflow();
    let (proc, scher) = create_proc(&mut workflow, "job1");

    proc.start();
    scher.on_proc(|proc: &Proc, _data: &EventData| {
        assert_eq!(proc.state(), TaskState::Running);
    });
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Proc, Arc<Scheduler>) {
    let scher = Scheduler::new();

    workflow.set_biz_id(id);
    let task = scher.create_raw_proc(workflow);

    (task.clone(), Arc::new(scher))
}

fn create_workflow() -> Workflow {
    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
