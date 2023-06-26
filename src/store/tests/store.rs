use crate::{
    event::EventAction,
    sch::{ActKind, NodeKind},
    store::{data, DbSet, Store, StoreKind},
    utils, Query, StoreAdapter, TaskState, Workflow,
};
use data::{Act, Proc, Task};
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<Store> = OnceCell::const_new();
async fn init() -> Store {
    let s = Store::new();
    s
}

async fn store() -> &'static Store {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn store_local() {
    let store = store().await;

    #[cfg(not(feature = "sqlite"))]
    assert_eq!(store.kind(), StoreKind::Local);

    #[cfg(feature = "sqlite")]
    assert_eq!(store.kind(), StoreKind::Sqlite);
}

#[tokio::test]
async fn store_load() {
    let store = store().await;

    let prefix = utils::shortid();
    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::None, &workflow);
        store.procs().create(&proc).expect("create proc");
    }

    let q = Query::new().set_limit(10000);
    let procs = store.procs().query(&q).unwrap();
    let procs = procs
        .iter()
        .filter(|it| it.pid.starts_with(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(procs.len(), 100);
}

#[tokio::test]
async fn store_model_deploy() {
    let store = store().await;
    let workflow = create_workflow();
    let ok = store.deploy(&workflow).unwrap();
    assert_eq!(ok, true);
}

#[tokio::test]
async fn store_model_deploy_ver_incr() {
    let store = store().await;
    let mut workflow = create_workflow();
    workflow.id = utils::longid();
    store.deploy(&workflow).unwrap();
    let model = store.models().find(&workflow.id).unwrap();

    assert_eq!(model.ver, 1);
    store.deploy(&workflow).unwrap();
    let model = store.models().find(&workflow.id).unwrap();
    assert_eq!(model.ver, 2);
}

#[tokio::test]
async fn store_models() {
    let store = store().await;

    let mut workflow = create_workflow();
    workflow.id = utils::longid();
    store.deploy(&workflow).unwrap();

    workflow.id = utils::longid();
    store.deploy(&workflow).unwrap();

    let q = Query::new().set_limit(2);
    let models = store.models().query(&q).unwrap();

    assert_eq!(models.len(), 2);
}

#[tokio::test]
async fn store_model_get() {
    let store = store().await;
    let mut workflow = create_workflow();
    workflow.id = utils::longid();
    store.deploy(&workflow).unwrap();

    let model = store.models().find(&workflow.id).unwrap();
    assert_eq!(model.id, workflow.id);
}

#[tokio::test]
async fn store_model_remove() {
    let store = store().await;

    let id = utils::longid();
    let mut workflow = create_workflow();
    workflow.id = id.clone();
    store.deploy(&workflow).unwrap();

    let model = store.models().find(&id);
    assert_eq!(model.is_ok(), true);

    store.models().delete(&id).unwrap();
    let model = store.models().find(&id);
    assert_eq!(model.is_err(), true);
}

#[tokio::test]
async fn store_model_deploy_id_error() {
    let store = store().await;
    let mut workflow = create_workflow();
    workflow.id = "".to_string();
    let result = store.deploy(&workflow);

    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn store_procs() {
    let store = store().await;
    let id = utils::longid();
    let workflow = create_workflow();
    let proc = create_proc(&id, TaskState::None, &workflow);
    store.procs().create(&proc).expect("create proc");

    let q = Query::new().set_limit(1);
    let procs = store.procs().query(&q).unwrap();
    assert_eq!(procs.len(), 1);
}

#[tokio::test]
async fn store_proc() {
    let store = store().await;

    let id = utils::longid();
    let workflow = create_workflow();
    let proc = create_proc(&id, TaskState::None, &workflow);
    store.procs().create(&proc).expect("create proc");
    let info = store.procs().find(&id).unwrap();
    assert_eq!(proc.pid, info.pid);
}

#[tokio::test]
async fn store_proc_update() {
    let store = store().await;

    let id = utils::longid();
    let workflow = create_workflow();
    let mut proc = create_proc(&id, TaskState::None, &workflow);

    store.procs().create(&proc).expect("create proc");

    proc.state = TaskState::Running.to_string();
    store.procs().update(&proc).expect("update proc");

    let p = store.procs().find(&proc.pid).unwrap();
    assert_eq!(p.pid, proc.pid);
    assert_eq!(p.state, TaskState::Running.to_string());
}

#[tokio::test]
async fn store_proc_remove() {
    let store = store().await;

    let id = utils::longid();
    let workflow = create_workflow();
    let proc = create_proc(&id, TaskState::None, &workflow);

    store.procs().create(&proc).expect("create proc");

    let proc = store.procs().find(&id);
    assert_eq!(proc.is_ok(), true);

    store.procs().delete(&id).unwrap();
    let proc = store.procs().find(&id);
    assert_eq!(proc.is_ok(), false);
}

#[tokio::test]
async fn store_task_create() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let nid = utils::shortid();
    let task = Task {
        id: format!("{pid}:{tid}"),
        kind: NodeKind::Step.to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
    };

    store.tasks().create(&task).expect("create task");

    let id = utils::Id::new(&pid, &tid);
    let ret = store.tasks().find(&id.id());
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_task_update() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let nid = utils::shortid();
    let task = Task {
        id: format!("{pid}:{tid}"),
        kind: NodeKind::Step.to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
    };

    store.tasks().create(&task).expect("create task");

    let id = utils::Id::new(&pid, &tid);
    let mut task = store.tasks().find(&id.id()).unwrap();
    task.state = TaskState::Running.to_string();
    store.tasks().update(&task).unwrap();

    let task2 = store.tasks().find(&id.id()).unwrap();
    assert_eq!(task.state, task2.state);
}

#[tokio::test]
async fn store_task_remove() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let nid = utils::shortid();
    let task = Task {
        id: format!("{pid}:{tid}"),
        kind: NodeKind::Step.to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
    };

    store.tasks().create(&task).expect("create task");
    store.tasks().delete(&task.id).expect("remove proc");

    let ret = store.tasks().find(&task.id);
    assert!(ret.is_err());
}

#[tokio::test]
async fn store_act_create() {
    let store = store().await;

    let id = utils::longid();
    let time = utils::time::time();
    let act = Act {
        id: id.clone(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        start_time: time,
        end_time: 0,
        state: "none".to_string(),
        active: false,
    };

    store.acts().create(&act).expect("create message");
    let ret = store.acts().find(&id).unwrap();
    assert_eq!(ret.active, false);
    assert_eq!(ret.vars, "{}");
    assert_eq!(ret.start_time, time);
}

#[tokio::test]
async fn store_act_update() {
    let store = store().await;

    let id = utils::longid();
    let mut act = Act {
        id: id.clone(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        start_time: 0,
        end_time: 0,
        state: "none".to_string(),
        active: false,
    };

    store.acts().create(&act).expect("create act");
    act.vars = "vars".to_string();
    act.end_time = 1000;
    act.active = true;
    let ret = store.acts().update(&act);
    let new_act = store.acts().find(&id).unwrap();
    assert!(ret.is_ok());
    assert_eq!(new_act.active, true);
    assert_eq!(new_act.vars, "vars");
    assert_eq!(new_act.end_time, 1000);
}

#[tokio::test]
async fn store_act_delete() {
    let store = store().await;

    let id = utils::longid();
    let mut act = Act {
        id: id.clone(),
        kind: ActKind::User.to_string(),
        event: EventAction::Create.to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        vars: "{}".to_string(),
        start_time: 0,
        end_time: 0,
        state: "none".to_string(),
        active: false,
    };

    store.acts().create(&act).unwrap();
    act.vars = "vars".to_string();
    act.end_time = 1000;
    let ret = store.acts().delete(&id);
    assert!(ret.is_ok());
    let ret = store.acts().find(&id);
    assert!(ret.is_err());
}

fn create_workflow() -> Workflow {
    let text = include_str!("../../../examples/store_test.yml");
    let workflow = Workflow::from_str(text).unwrap();

    workflow
}

fn create_proc(id: &str, state: TaskState, model: &Workflow) -> Proc {
    Proc {
        id: id.to_string(),
        pid: id.to_string(),
        model: model.to_string().unwrap(),
        state: state.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
    }
}

#[derive(Debug)]
struct TestStore;

impl StoreAdapter for TestStore {
    fn models(&self) -> Arc<dyn DbSet<Item = data::Model>> {
        todo!()
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = data::Proc>> {
        todo!()
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = data::Task>> {
        todo!()
    }

    fn acts(&self) -> Arc<dyn DbSet<Item = data::Act>> {
        todo!()
    }

    fn init(&self) {}

    fn flush(&self) {}
}
