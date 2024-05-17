use crate::{
    sch::NodeKind,
    store::{data, query::Expr, Cond, Store, StoreKind},
    utils, Query, StoreAdapter, TaskState, Workflow,
};
use data::{Message, MessageStatus, Package, Proc, Task};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<Arc<Store>> = OnceCell::const_new();
async fn init() -> Arc<Store> {
    #[cfg(feature = "store")]
    {
        return Arc::new(Store::local("test_data", "test.db"));
    }

    #[cfg(not(feature = "store"))]
    Store::default()
}

async fn store() -> &'static Arc<Store> {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn store_kind() {
    let store = store().await;
    #[cfg(feature = "store")]
    assert_eq!(store.kind(), StoreKind::Local);
    #[cfg(not(feature = "store"))]
    assert_eq!(store.kind(), StoreKind::Memory);
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
        pid: pid.clone(),
        tid: tid.clone(),
        node_data: nid,
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
        pid: pid.clone(),
        tid: tid.clone(),
        node_data: nid,
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
        pid: pid.clone(),
        tid: tid.clone(),
        node_data: nid,
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

#[tokio::test]
async fn store_message_create() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        state: "created".to_string(),
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        source: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).expect("create message");

    let id = utils::Id::new(&pid, &tid);
    let ret = store.messages().find(&id.id());
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_message_query() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        state: "created".to_string(),
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        source: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).unwrap();

    let id = utils::Id::new(&pid, &tid);
    let q = Query::new().push(Cond::and().push(Expr::eq("id", id.id())));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_message_update() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        state: "created".to_string(),
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        source: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).unwrap();

    let id = utils::Id::new(&pid, &tid);
    let mut msg = store.messages().find(&id.id()).unwrap();
    msg.state = "completed".to_string();
    msg.retry_times = 1;
    msg.status = MessageStatus::Completed;
    store.messages().update(&msg).unwrap();

    let msg2 = store.messages().find(&id.id()).unwrap();
    assert_eq!(msg2.state, "completed");
    assert_eq!(msg2.retry_times, 1);
    assert_eq!(msg2.status, MessageStatus::Completed);
}

#[tokio::test]
async fn store_message_remove() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        state: "created".to_string(),
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        source: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).unwrap();
    store.messages().delete(&msg.id).unwrap();

    let ret = store.messages().find(&msg.id);
    assert!(ret.is_err());
}

#[tokio::test]
async fn store_package_create() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        name: "test package".to_string(),
        size: 100,
        file_data: vec![0x01, 0x02],
        create_time: 0,
        update_time: 0,
        timestamp: 0,
    };

    store.packages().create(&package).unwrap();
    let ret = store.packages().find(&package.id);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_package_query() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        name: "test package".to_string(),
        size: 100,
        file_data: vec![0x01, 0x02],
        create_time: 0,
        update_time: 0,
        timestamp: 0,
    };
    store.packages().create(&package).unwrap();
    let q = Query::new().push(Cond::and().push(Expr::eq("id", package.id)));
    let ret = store.packages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_package_update() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        name: "test package".to_string(),
        size: 100,
        file_data: vec![0x01, 0x02],
        create_time: 0,
        update_time: 0,
        timestamp: 0,
    };
    store.packages().create(&package).unwrap();
    let mut p = store.packages().find(&package.id).unwrap();
    p.name = "my name".to_string();
    p.size = 200;
    p.file_data = vec![0x02, 0x03];
    store.packages().update(&p).unwrap();

    let p2 = store.packages().find(&package.id).unwrap();
    assert_eq!(p2.name, "my name");
    assert_eq!(p2.size, 200);
    assert_eq!(p2.file_data, vec![0x02, 0x03]);
}

#[tokio::test]
async fn store_package_remove() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        name: "test package".to_string(),
        size: 100,
        file_data: vec![0x01, 0x02],
        create_time: 0,
        update_time: 0,
        timestamp: 0,
    };
    store.packages().create(&package).unwrap();
    store.packages().delete(&package.id).unwrap();

    let ret = store.packages().find(&package.id);
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
        env_local: "{}".to_string(),
        err: None,
    }
}
