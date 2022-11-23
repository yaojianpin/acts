use crate::{
    sch::{Proc, Scheduler},
    TaskState, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn sch_next() {
    let scher = Scheduler::new();
    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let s = scher.clone();
    tokio::spawn(async move {
        s.push(&workflow);
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}

#[tokio::test]
async fn sch_schedule_task() {
    let scher = Arc::new(Scheduler::new());

    let text = include_str!("./simple.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let s = scher.clone();
    tokio::spawn(async move {
        let proc = Proc::new(s.clone(), &workflow, &TaskState::Pending);
        s.sched_proc(&proc)
    });

    let ret = scher.next().await;
    assert_eq!(ret, true);
}
