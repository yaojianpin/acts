use crate::{
    sch::NodeKind,
    store::{data::*, db::LocalStore, Query},
    utils, StoreAdapter, TaskState, Vars,
};
use std::collections::HashMap;
use tokio::sync::OnceCell;

static STORE: OnceCell<LocalStore> = OnceCell::const_new();

async fn init() -> LocalStore {
    let s = LocalStore::new();
    s
}

async fn store() -> &'static LocalStore {
    STORE.get_or_init(init).await
}

// #[tokio::test]
// async fn local_init() {
//     let store = store().await;
//     assert_eq!(store.is_initialized(), true);
// }

#[tokio::test]
async fn local_proc_create() {
    let store = store().await;
    let proc = Proc {
        id: utils::longid(),
        pid: "pid".to_string(),
        model: "".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().exists(&proc.id), true);
}

#[tokio::test]
async fn local_proc_query() {
    let store = store().await;
    let procs = store.procs();
    let pid = utils::shortid();
    for _ in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            pid: pid.to_string(),
            model: "".to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            vars: "".to_string(),
        };
        procs.create(&proc).unwrap();
    }

    let q = Query::new().push("pid", &pid).set_limit(5);
    let items = procs.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn local_proc_update() {
    let store = store().await;

    let mut vars: Vars = HashMap::new();
    vars.insert("k1".to_string(), "v1".into());

    let mut proc = Proc {
        id: utils::shortid(),
        pid: "pid".to_string(),
        model: "".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).unwrap();

    proc.state = TaskState::Running.into();
    proc.vars = serde_yaml::to_string(&vars).unwrap();
    store.procs().update(&proc).unwrap();

    let p = store.procs().find(&proc.id).unwrap();
    assert_eq!(p.state, proc.state);
    assert_eq!(p.vars, proc.vars);
}

#[tokio::test]
async fn local_proc_delete() {
    let store = store().await;
    let proc = Proc {
        id: utils::shortid(),
        pid: "pid".to_string(),
        model: "".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    };
    store.procs().create(&proc).unwrap();
    store.procs().delete(&proc.id).unwrap();

    assert_eq!(store.procs().exists(&proc.id), false);
}

#[tokio::test]
async fn local_task_create() {
    let store = store().await;
    let tasks = store.tasks();
    let task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        nid: "nid".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        uid: "".to_string(),
    };
    tasks.create(&task).unwrap();
    assert_eq!(tasks.exists(&task.id), true);
}

#[tokio::test]
async fn local_task_query() {
    let store = store().await;
    let tasks = store.tasks();
    let pid = utils::shortid();
    for _ in 0..5 {
        let task = Task {
            kind: NodeKind::Workflow.into(),
            id: utils::shortid(),
            pid: pid.to_string(),
            tid: "tid".to_string(),
            nid: "nid".to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            uid: "".to_string(),
        };
        tasks.create(&task).unwrap();
    }

    let q = Query::new().push("pid", &pid).set_limit(5);
    let items = tasks.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn local_task_update() {
    let store = store().await;
    let table = store.tasks();
    let mut task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        nid: "nid".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        uid: "".to_string(),
    };
    table.create(&task).unwrap();

    task.state = TaskState::Running.into();
    table.update(&task).unwrap();

    let t = table.find(&task.id).unwrap();
    assert_eq!(t.state, task.state);
}

#[tokio::test]
async fn local_task_delete() {
    let store = store().await;
    let table = store.tasks();
    let task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        nid: "nid".to_string(),
        state: TaskState::None.into(),
        start_time: 0,
        end_time: 0,
        uid: "".to_string(),
    };
    table.create(&task).unwrap();
    table.delete(&task.id).unwrap();

    assert_eq!(table.exists(&task.id), false);
}

#[tokio::test]
async fn local_message_create() {
    let store = store().await;
    let table = store.messages();
    let vars = utils::vars::to_string(&Vars::new());
    let msg = Message {
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "u1".to_string(),
        vars,
        state: 0,
        create_time: 0,
        update_time: 0,
    };
    table.create(&msg).unwrap();
    assert_eq!(table.exists(&msg.id), true);
}

#[tokio::test]
async fn local_message_query() {
    let store = store().await;
    let messages = store.messages();
    let pid = utils::shortid();
    let vars = utils::vars::to_string(&Vars::new());
    for _ in 0..5 {
        let msg = Message {
            id: utils::shortid(),
            pid: pid.to_string(),
            tid: "tid".to_string(),
            uid: "u1".to_string(),
            vars: vars.clone(),
            state: 0,
            create_time: 0,
            update_time: 0,
        };
        messages.create(&msg).unwrap();
    }

    let q = Query::new().push("pid", &pid).set_limit(5);
    let items = messages.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn local_message_update() {
    let store = store().await;
    let table = store.messages();
    let vars = utils::vars::to_string(&Vars::new());
    let mut msg = Message {
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "u1".to_string(),
        vars,
        state: 0,
        create_time: 0,
        update_time: 0,
    };
    table.create(&msg).unwrap();

    msg.uid = "u2".to_string();
    table.update(&msg).unwrap();

    let t = table.find(&msg.id).unwrap();
    assert_eq!(t.uid, msg.uid);
}

#[tokio::test]
async fn local_message_delete() {
    let store = store().await;
    let table = store.messages();
    let vars = utils::vars::to_string(&Vars::new());
    let msg = Message {
        id: utils::shortid(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "u1".to_string(),
        vars,
        state: 0,
        create_time: 0,
        update_time: 0,
    };
    table.create(&msg).unwrap();
    table.delete(&msg.id).unwrap();

    assert_eq!(table.exists(&msg.id), false);
}
