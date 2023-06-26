use crate::{
    event::EventAction,
    sch::{ActKind, NodeKind},
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

// #[tokio::test(flavor = "multi_thread" )]
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
    assert_eq!(store.procs().exists(&proc.id).unwrap(), true);
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

    assert_eq!(store.procs().exists(&proc.id).unwrap(), false);
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
    };
    tasks.create(&task).unwrap();
    assert_eq!(tasks.exists(&task.id).unwrap(), true);
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
    };
    table.create(&task).unwrap();
    table.delete(&task.id).unwrap();

    assert_eq!(table.exists(&task.id).unwrap(), false);
}

#[tokio::test]
async fn local_act_create() {
    let store = store().await;
    let table = store.acts();
    let vars = utils::vars::to_string(&Vars::new());
    let act = Act {
        id: utils::shortid(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        create_time: 0,
        update_time: 0,
        state: "none".to_string(),
        ack: false,
        active: false,
    };
    table.create(&act).unwrap();
    assert_eq!(table.exists(&act.id).unwrap(), true);
}

#[tokio::test]
async fn local_act_query() {
    let store = store().await;
    let acts = store.acts();
    let pid = utils::shortid();
    let vars = utils::vars::to_string(&Vars::new());
    for _ in 0..5 {
        let act = Act {
            id: utils::shortid(),
            kind: ActKind::User.to_string(),
            event: EventAction::Create.to_string(),
            pid: "pid".to_string(),
            tid: "tid".to_string(),
            vars: "{}".to_string(),
            create_time: 0,
            update_time: 0,
            state: "none".to_string(),
            ack: false,
            active: false,
        };
        acts.create(&act).unwrap();
    }

    let q = Query::new().push("pid", &pid).set_limit(5);
    let items = acts.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn local_act_update() {
    let store = store().await;
    let table = store.acts();
    let vars = utils::vars::to_string(&Vars::new());
    let mut act = Act {
        id: utils::shortid(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        create_time: 0,
        update_time: 0,
        state: "none".to_string(),
        ack: false,
        active: false,
    };
    table.create(&act).unwrap();

    act.vars = "u2".to_string();
    table.update(&act).unwrap();

    let t = table.find(&act.id).unwrap();
    assert_eq!(t.vars, act.vars);
}

#[tokio::test]
async fn local_act_delete() {
    let store = store().await;
    let table = store.acts();
    let vars = utils::vars::to_string(&Vars::new());
    let act = Act {
        id: utils::shortid(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        create_time: 0,
        update_time: 0,
        state: "none".to_string(),
        ack: false,
        active: false,
    };
    table.create(&act).unwrap();
    table.delete(&act.id).unwrap();

    assert_eq!(table.exists(&act.id).unwrap(), false);
}
