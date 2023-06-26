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
    let store = engine.store();
    let scher = engine.scher();
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let s = scher.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        let mut options = Vars::new();
        options.insert("biz_id".to_string(), json!(utils::longid()));
        s.start(&workflow, &options).unwrap();
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_scher_task() {
    let scher = Scheduler::new();

    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let pid = utils::longid();
    let s = scher.clone();
    tokio::spawn(async move {
        let proc = Arc::new(Proc::new(&pid, &workflow, &TaskState::Pending));
        s.sched_proc(&proc)
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}
