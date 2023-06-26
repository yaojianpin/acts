use crate::{
    sch::{Act, ActKind, NodeTree, Proc, Scheduler},
    utils, Vars, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_proc_task() {
    let mut workflow = create_workflow();
    let id = utils::longid();

    let tr = NodeTree::build(&mut workflow);
    let (proc, _) = create_proc(&mut workflow, &id);

    let node = tr.root.as_ref().unwrap();
    let task = proc.create_task(node, None);
    assert!(proc.task(&task.tid).is_some())
}

#[tokio::test]
async fn sch_proc_acts() {
    let mut workflow = create_workflow();
    let id = utils::longid();

    let tr = NodeTree::build(&mut workflow);
    let (proc, scher) = create_proc(&mut workflow, &id);

    let task = proc.create_task(tr.root.as_ref().unwrap(), None);
    scher.sched_proc(&proc);
    let act = Act::new(&task, ActKind::Action, &Vars::new());
    proc.push_act(&act);
    assert!(scher.act(&proc.pid(), &act.id).is_some())
}

fn create_proc(workflow: &mut Workflow, id: &str) -> (Arc<Proc>, Arc<Scheduler>) {
    let scher = Scheduler::new();
    let proc = scher.create_proc(id, workflow);
    (proc, scher)
}

fn create_workflow() -> Workflow {
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    workflow
}
