use crate::{
    sch::{Proc, Scheduler},
    utils, Vars, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn proc_messages() {
    let mut workflow = create_workflow();
    let id = utils::longid();
    let (proc, scher) = create_proc(&mut workflow, &id);
    scher.sched_proc(&proc);
    let tid = utils::shortid();
    let msg = proc.make_message(&tid, Some("u1".to_string()), Vars::new());
    assert!(scher.message(&msg.id).is_some())
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Scheduler>) {
    let scher = Scheduler::new();
    let proc = Arc::new(scher.create_raw_proc(id, workflow));

    scher.cache().create_proc(&proc);
    (proc, scher)
}

fn create_workflow() -> Workflow {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
