use crate::{
    sch::{Proc, Scheduler},
    utils, Engine, TaskState, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_next() {
    let engine = Engine::new();
    engine.start();
    let store = engine.store();
    let scher = engine.scher();
    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let s = scher.clone();
    store.deploy(&workflow).unwrap();
    tokio::spawn(async move {
        s.start(
            &workflow,
            crate::ActionOptions {
                biz_id: Some(utils::longid()),
                ..Default::default()
            },
        )
        .unwrap();
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_schedule_task() {
    let scher = Arc::new(Scheduler::new());

    let text = include_str!("./models/simple.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let pid = utils::longid();
    let s = scher.clone();
    tokio::spawn(async move {
        let proc = Proc::new(&pid, s.clone(), &workflow, &TaskState::Pending);
        s.sched_proc(&proc)
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}
