use crate::{
    event::{Emitter, EventData},
    sch::{Proc, Scheduler},
    Engine, TaskState, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_task_state() {
    let mut workflow = create_simple_workflow();
    let (proc, _, _) = create_proc(&mut workflow, "w1");
    assert!(proc.state() == TaskState::None);
}

#[tokio::test]
async fn sch_task_start() {
    let mut workflow = create_simple_workflow();
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    proc.start(&scher);
    emitter.on_proc(|proc: &Arc<Proc>, _data: &EventData| {
        assert_eq!(proc.state(), TaskState::Running);
    });
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = scher.create_raw_proc(pid, workflow);

    let emitter = scher.emitter();

    (Arc::new(proc), scher, emitter)
}

fn create_simple_workflow() -> Workflow {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
