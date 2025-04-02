use crate::{
    cache::Cache,
    data,
    scheduler::{NodeTree, Process, TaskState},
    store::StoreKind,
    utils, Engine, Workflow,
};

#[tokio::test]
async fn cache_new() {
    let cache = Cache::new(1);
    assert_eq!(cache.cap(), 1);
    assert_eq!(cache.store().kind(), StoreKind::Memory);
}

#[tokio::test]
async fn cache_count() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let cache = Cache::new(10);

    let proc = Process::new(&utils::longid(), &rt);
    cache.push_proc(&proc);
    assert_eq!(cache.count(), 1);
}

#[tokio::test]
async fn cache_push_get() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let cache = Cache::new(10);
    let pid = utils::longid();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc);
    assert_eq!(cache.count(), 1);

    let proc = cache.proc(&pid, &engine.runtime());
    assert!(proc.is_some());
}

#[tokio::test]
async fn cache_push_to_store() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let cache = Cache::new(1);

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc);
        pids.push(pid);
    }

    assert_eq!(cache.count(), 1);
    for pid in pids.iter() {
        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert!(exists);
    }
}

#[tokio::test]
async fn cache_remove() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let cache = Cache::new(10);

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc);
        pids.push(pid);
    }

    assert_eq!(cache.count(), 5);
    for pid in pids.iter() {
        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert!(exists);

        cache.remove(pid).unwrap();
        assert!(cache.proc(pid, &engine.runtime()).is_none());

        let exists = cache.store().base().procs().exists(pid).unwrap();
        assert!(!exists);
    }
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn cache_upsert() {
    let engine = Engine::new();
    let rt = engine.runtime();
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tree = NodeTree::build(&mut workflow).unwrap();

    let cache = Cache::new(10);
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc);
    assert_eq!(cache.count(), 1);

    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None);

    proc.set_state(TaskState::Running);
    cache.upsert(&task).unwrap();

    let proc = cache.proc(&pid, &engine.runtime()).unwrap();
    assert_eq!(proc.state(), TaskState::Running);
}

#[tokio::test]
async fn cache_restore_count() {
    let engine = Engine::new();
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
            timestamp: 0,
            model: model.to_json().unwrap(),
            env_local: "{}".to_string(),
            err: None,
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(&engine.runtime(), |proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 5);
}

#[tokio::test]
async fn cache_restore_working_state() {
    let engine = Engine::new();
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
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env_local: "{}".to_string(),
            err: None,
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(&engine.runtime(), |proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 5);
}

#[tokio::test]
async fn cache_restore_completed_state() {
    let engine = Engine::new();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [
        TaskState::Skipped,
        TaskState::Skipped,
        TaskState::Skipped,
        TaskState::Aborted,
        TaskState::Aborted,
        TaskState::Aborted,
        TaskState::Error,
        TaskState::Error,
        TaskState::Completed,
        TaskState::Completed,
    ];
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env_local: "{}".to_string(),
            err: None,
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(&engine.runtime(), |proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn cache_restore_less_cap() {
    let engine = Engine::new();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let cache = Cache::new(5);
    cache.store().deploy(&model).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [TaskState::Running, TaskState::None, TaskState::Pending];
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env_local: "{}".to_string(),
            err: None,
        };
        cache.store().base().procs().create(&proc).unwrap();
    }

    cache
        .restore(&engine.runtime(), |proc| {
            println!("on_load: {:?}", proc);
        })
        .unwrap();
    assert_eq!(cache.count(), 3);
}
