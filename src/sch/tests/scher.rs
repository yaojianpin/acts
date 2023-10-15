use crate::{
    sch::{Proc, Scheduler},
    utils, Engine, TaskState, Vars, Workflow,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sch_scher_next() {
    let engine = Engine::new();
    engine.start();
    let store = engine.scher().cache().store();
    let scher = engine.scher();
    let workflow = Workflow::new().with_id(&utils::longid());

    let s = scher.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        let mut options = Vars::new();
        options.insert("pid".to_string(), json!(utils::longid()));
        s.start(&workflow, &options).unwrap();
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_scher_task() {
    let scher = Scheduler::new();
    let workflow = Workflow::new();
    let pid = utils::longid();
    let s = scher.clone();
    tokio::spawn(async move {
        let proc = Proc::new(&pid);
        proc.load(&workflow);
        proc.set_state(TaskState::Pending);
        s.launch(&Arc::new(proc))
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}
