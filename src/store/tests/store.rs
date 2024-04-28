use crate::{
    sch::NodeKind,
    store::{data, Cond, DbSet, Expr, Store, StoreKind},
    utils, Query, StoreAdapter, TaskState, Workflow,
};
use data::{Proc, Task};
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<Store> = OnceCell::const_new();
async fn init() -> Store {
    let s = Store::local("test_data", "test.db");
    s
}

async fn store() -> &'static Store {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn store_local() {
    let store = store().await;
    assert_eq!(store.kind(), StoreKind::Local);
}

#[tokio::test]
async fn store_load_by_limit() {
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
        .filter(|it| it.id.starts_with(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(procs.len(), 100);
}

#[tokio::test]
async fn store_load_by_state() {
    let store = store().await;

    let prefix = utils::shortid();
    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::Running, &workflow);
        store.procs().create(&proc).expect("create proc");
    }

    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::Pending, &workflow);
        store.procs().create(&proc).expect("create proc");
    }

    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::Completed, &workflow);
        store.procs().create(&proc).expect("create proc");
    }

    let q = Query::new()
        .push(
            Cond::or()
                .push(Expr::eq("state", "running"))
                .push(Expr::eq("state", "pending")),
        )
        .set_limit(10000);
    let procs = store.procs().query(&q).unwrap();
    let procs = procs
        .iter()
        .filter(|it| it.id.starts_with(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(procs.len(), 200);
}

#[tokio::test]
async fn store_model_deploy_ok() {
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
    assert_eq!(proc.id, info.id);
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

    let p = store.procs().find(&proc.id).unwrap();
    assert_eq!(p.id, proc.id);
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
        name: "test".to_string(),
        prev: None,
        kind: NodeKind::Step.to_string(),
        proc_id: pid.clone(),
        task_id: tid.clone(),
        node_id: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
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
        name: "test".to_string(),
        prev: None,
        kind: NodeKind::Step.to_string(),
        proc_id: pid.clone(),
        task_id: tid.clone(),
        node_id: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
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
        name: "test".to_string(),
        prev: None,
        kind: NodeKind::Step.to_string(),
        proc_id: pid.clone(),
        task_id: tid.clone(),
        node_id: nid,
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
    };

    store.tasks().create(&task).expect("create task");
    store.tasks().delete(&task.id).expect("remove proc");

    let ret = store.tasks().find(&task.id);
    assert!(ret.is_err());
}

fn create_workflow() -> Workflow {
    Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"))
}

fn create_proc(id: &str, state: TaskState, model: &Workflow) -> Proc {
    Proc {
        id: id.to_string(),
        name: model.name.clone(),
        mid: model.id.clone(),
        state: state.to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: utils::time::timestamp(),
        model: model.to_json().unwrap(),
        root_tid: "".to_string(),
        env_local: "{}".to_string(),
        err: None,
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

    fn packages(&self) -> Arc<dyn DbSet<Item = data::Package>> {
        todo!()
    }

    fn init(&self) {}
    fn close(&self) {}
}
