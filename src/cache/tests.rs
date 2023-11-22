use crate::{
    cache::Cache,
    data,
    sch::{NodeTree, Proc, TaskState},
    store::StoreKind,
    utils, Workflow,
};
use std::sync::Arc;

#[test]
fn cache_new() {
    let cache = Cache::new(1);
    assert_eq!(cache.cap(), 1);
    assert_eq!(cache.store().kind(), StoreKind::Memory);
}

#[test]
fn cache_count() {
    let cache = Cache::new(10);

    let proc = Arc::new(Proc::new(&utils::longid()));
    cache.push(&proc);
    assert_eq!(cache.count(), 1);
}

#[test]
fn cache_push_get() {
    let cache = Cache::new(10);
    let pid = utils::longid();
    let proc = Arc::new(Proc::new(&pid));
    cache.push(&proc);
    assert_eq!(cache.count(), 1);

    let proc = cache.proc(&pid);
    assert_eq!(proc.is_some(), true);
}

#[test]
fn cache_push_to_store() {
    let cache = Cache::new(1);

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Arc::new(Proc::new(&pid));
        cache.push(&proc);
        pids.push(pid);
    }

    assert_eq!(cache.count(), 1);
    for pid in pids.iter() {
        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert_eq!(exists, true);
    }
}

#[test]
fn cache_remove() {
    let cache = Cache::new(10);

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Arc::new(Proc::new(&pid));
        cache.push(&proc);
        pids.push(pid);
    }

    assert_eq!(cache.count(), 5);
    for pid in pids.iter() {
        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert_eq!(exists, true);

        cache.remove(pid).unwrap();
        assert_eq!(cache.proc(pid).is_none(), true);

        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert_eq!(exists, false);
    }
    assert_eq!(cache.count(), 0);
}

#[test]
fn cache_upsert() {
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tree = NodeTree::build(&mut workflow).unwrap();

    let cache = Cache::new(10);
    let proc = Arc::new(Proc::new(&pid));
    cache.push(&proc);
    assert_eq!(cache.count(), 1);

    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None);

    proc.set_state(TaskState::Running);
    cache.upsert(&task).unwrap();

    let proc = cache.proc(&pid).unwrap();
    assert_eq!(proc.state(), TaskState::Running);
}

#[test]
fn cache_restore_count() {
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);
    for _ in 0..10 {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            vars: "".to_string(),
            timestamp: 0,
            model: model.to_json().unwrap(),
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(|proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 5);
}

#[test]
fn cache_restore_working_state() {
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [
        TaskState::None,
        TaskState::None,
        TaskState::None,
        TaskState::Running,
        TaskState::Running,
        TaskState::Running,
        TaskState::Pending,
        TaskState::Pending,
        TaskState::Pending,
        TaskState::Pending,
    ];
    for i in 0..10 {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: states[i].to_string(),
            start_time: 0,
            end_time: 0,
            vars: "".to_string(),
            timestamp: 0,
            model: model.to_json().unwrap(),
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(|proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 5);
}

#[test]
fn cache_restore_completed_state() {
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [
        TaskState::Skip,
        TaskState::Skip,
        TaskState::Skip,
        TaskState::Abort,
        TaskState::Abort,
        TaskState::Abort,
        TaskState::Fail(format!("err")),
        TaskState::Fail(format!("err")),
        TaskState::Success,
        TaskState::Success,
    ];
    for i in 0..10 {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: states[i].to_string(),
            start_time: 0,
            end_time: 0,
            vars: "".to_string(),
            timestamp: 0,
            model: model.to_json().unwrap(),
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(|proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 0);
}

#[test]
fn cache_restore_less_cap() {
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [TaskState::Running, TaskState::None, TaskState::Pending];
    for i in 0..3 {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: states[i].to_string(),
            start_time: 0,
            end_time: 0,
            vars: "".to_string(),
            timestamp: 0,
            model: model.to_json().unwrap(),
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(|proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 3);
}
