use crate::{
    sch::NodeKind,
    store::{data::*, db::MemStore, Cond, Expr, Query},
    utils, StoreAdapter, TaskState, Vars,
};
use tokio::sync::OnceCell;

static STORE: OnceCell<MemStore> = OnceCell::const_new();
async fn init() -> MemStore {
    let s = MemStore::new();
    s
}

async fn store() -> &'static MemStore {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn store_mem_proc_create() {
    let store = store().await;
    let proc = Proc {
        id: utils::longid(),
        name: "name".to_string(),
        mid: "m1".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        root_tid: "".to_string(),
        env_local: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().exists(&proc.id).unwrap(), true);
}

#[tokio::test]
async fn store_mem_proc_find() {
    let store = store().await;
    let pid = utils::longid();
    let proc = Proc {
        id: pid.clone(),
        name: "name".to_string(),
        mid: "m1".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        root_tid: "".to_string(),
        env_local: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().find(&pid).unwrap().id, pid);
}

#[tokio::test]
async fn store_mem_proc_query() {
    let store = store().await;
    let procs = store.procs();
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: i.to_string(),
            mid: mid.to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: "".to_string(),
            root_tid: "".to_string(),
            env_local: "".to_string(),
            err: None,
        };
        procs.create(&proc).unwrap();
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("mid", &mid)))
        .set_limit(5);
    let items = procs.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn store_mem_proc_update() {
    let store = store().await;

    let mut vars: Vars = Vars::new();
    vars.insert("k1".to_string(), "v1".into());

    let mut proc = Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        root_tid: "".to_string(),
        env_local: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();

    proc.state = TaskState::Running.into();
    store.procs().update(&proc).unwrap();

    let p = store.procs().find(&proc.id).unwrap();
    assert_eq!(p.state, proc.state);
}

#[tokio::test]
async fn store_mem_proc_delete() {
    let store = store().await;
    let proc = Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        root_tid: "".to_string(),
        env_local: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    store.procs().delete(&proc.id).unwrap();

    assert_eq!(store.procs().exists(&proc.id).unwrap(), false);
}

#[tokio::test]
async fn store_mem_task_create() {
    let store = store().await;
    let tasks = store.tasks();
    let task = Task {
        id: utils::shortid(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        proc_id: "pid".to_string(),
        task_id: "tid".to_string(),
        node_id: "nid".to_string(),
        state: TaskState::None.into(),
        prev: None,
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
    };
    tasks.create(&task).unwrap();
    assert_eq!(tasks.exists(&task.id).unwrap(), true);
}

#[tokio::test]
async fn store_mem_task_find() {
    let store = store().await;
    let tasks = store.tasks();
    let tid = utils::shortid();
    let task = Task {
        id: tid.clone(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        proc_id: "pid".to_string(),
        task_id: "tid".to_string(),
        node_id: "nid".to_string(),
        state: TaskState::None.into(),
        data: "{}".to_string(),
        prev: None,
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        err: None,
    };
    tasks.create(&task).unwrap();
    assert_eq!(tasks.find(&tid).unwrap().id, tid);
}

#[tokio::test]
async fn store_mem_task_query() {
    let store = store().await;
    let tasks = store.tasks();
    let pid = utils::shortid();
    for _ in 0..5 {
        let task = Task {
            kind: NodeKind::Workflow.into(),
            id: utils::shortid(),
            name: "test".to_string(),
            proc_id: pid.to_string(),
            task_id: "tid".to_string(),
            node_id: "nid".to_string(),
            state: TaskState::None.into(),
            prev: None,
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        tasks.create(&task).unwrap();
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("proc_id", &pid)))
        .set_limit(5);
    let items = tasks.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn store_mem_task_update() {
    let store = store().await;
    let table = store.tasks();
    let mut task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        name: "test".to_string(),
        proc_id: "pid".to_string(),
        task_id: "tid".to_string(),
        node_id: "nid".to_string(),
        state: TaskState::None.into(),
        prev: None,
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
    };
    table.create(&task).unwrap();

    task.state = TaskState::Completed.into();
    task.prev = Some("tid1".to_string());
    table.update(&task).unwrap();

    let t = table.find(&task.id).unwrap();
    assert_eq!(t.state, task.state);
    assert_eq!(t.prev, task.prev);
}

#[tokio::test]
async fn store_mem_task_delete() {
    let store = store().await;
    let table = store.tasks();
    let task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        name: "test".to_string(),
        proc_id: "pid".to_string(),
        task_id: "tid".to_string(),
        node_id: "nid".to_string(),
        state: TaskState::None.into(),
        prev: None,
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
    };
    table.create(&task).unwrap();
    table.delete(&task.id).unwrap();

    assert_eq!(table.exists(&task.id).unwrap(), false);
}
