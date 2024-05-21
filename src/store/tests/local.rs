use crate::{
    sch::NodeKind,
    store::{data::*, db::LocalStore, query::Expr, Cond, Query},
    utils, StoreAdapter, TaskState, Vars,
};
use serde_json::json;
use tokio::sync::OnceCell;

static STORE: OnceCell<LocalStore> = OnceCell::const_new();
async fn init() -> LocalStore {
    let s = LocalStore::new("test_data", "test.db");
    s
}

async fn store() -> &'static LocalStore {
    STORE.get_or_init(init).await
}

#[tokio::test]
async fn store_mem_model_create() {
    let store = store().await;
    let model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        time: 3333,
        data: "{}".to_string(),
    };
    store.models().create(&model).unwrap();
    assert_eq!(store.models().exists(&model.id).unwrap(), true);
}

#[tokio::test]
async fn store_mem_model_find() {
    let store = store().await;
    let mid: String = utils::longid();
    let model = Model {
        id: mid.clone(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        time: 3333,
        data: "{}".to_string(),
    };
    store.models().create(&model).unwrap();
    assert_eq!(store.models().find(&mid).unwrap().id, mid);
}

#[tokio::test]
async fn store_mem_model_query() {
    let store = store().await;
    let models = store.models();
    for _ in 0..5 {
        let model = Model {
            id: utils::longid(),
            name: "test_model".to_string(),
            ver: 1,
            size: 1245,
            time: 3333,
            data: "{}".to_string(),
        };
        models.create(&model).unwrap();
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("name", "test_model")))
        .set_limit(5);
    let items = models.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn store_mem_model_update() {
    let store = store().await;

    let mut model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        time: 3333,
        data: "{}".to_string(),
    };
    store.models().create(&model).unwrap();

    model.ver = 3;
    store.models().update(&model).unwrap();

    let p = store.models().find(&model.id).unwrap();
    assert_eq!(p.ver, model.ver);
}

#[tokio::test]
async fn store_mem_model_delete() {
    let store = store().await;
    let model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        time: 3333,
        data: "{}".to_string(),
    };
    store.models().create(&model).unwrap();
    store.models().delete(&model.id).unwrap();

    assert_eq!(store.procs().exists(&model.id).unwrap(), false);
}

#[tokio::test]
async fn store_local_proc_create() {
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
        env_local: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().exists(&proc.id).unwrap(), true);
}

#[tokio::test]
async fn store_local_proc_find() {
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
        env_local: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().find(&pid).unwrap().id, pid);
}

#[tokio::test]
async fn store_local_proc_query() {
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
            env_local: "{}".to_string(),
            err: None,
        };
        procs.create(&proc).unwrap();
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("mid", mid)))
        .set_limit(5);
    let items = procs.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn store_local_proc_update() {
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
        env_local: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();

    proc.state = TaskState::Running.into();
    proc.err = None;
    store.procs().update(&proc).unwrap();

    let p = store.procs().find(&proc.id).unwrap();
    assert_eq!(p.state, proc.state);
    assert_eq!(p.err, None);
}

#[tokio::test]
async fn store_local_proc_delete() {
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
        env_local: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    store.procs().delete(&proc.id).unwrap();

    assert_eq!(store.procs().exists(&proc.id).unwrap(), false);
}

#[tokio::test]
async fn store_local_task_create() {
    let store = store().await;
    let tasks = store.tasks();
    let task = Task {
        id: utils::shortid(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
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
async fn store_local_task_find() {
    let store = store().await;
    let tasks = store.tasks();
    let tid = utils::shortid();
    let task = Task {
        id: tid.clone(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
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
    assert_eq!(tasks.find(&tid).unwrap().id, tid);
}

#[tokio::test]
async fn store_local_task_query() {
    let store = store().await;
    let tasks = store.tasks();
    let pid = utils::shortid();
    for _ in 0..5 {
        let task = Task {
            kind: NodeKind::Workflow.into(),
            id: utils::shortid(),
            name: "test".to_string(),
            pid: pid.to_string(),
            tid: "tid".to_string(),
            node_data: "nid".to_string(),
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
        .push(Cond::and().push(Expr::eq("pid", pid)))
        .set_limit(5);
    let items = tasks.query(&q).unwrap();
    assert_eq!(items.len(), 5);
}

#[tokio::test]
async fn store_local_task_update() {
    let store = store().await;
    let table = store.tasks();
    let mut task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
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
async fn store_local_task_delete() {
    let store = store().await;
    let table = store.tasks();
    let task = Task {
        kind: NodeKind::Workflow.into(),
        id: utils::shortid(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
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

#[tokio::test]
async fn store_local_message_create() {
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
        timestamp: 0,
    };

    store.messages().create(&msg).expect("create message");

    let id = utils::Id::new(&pid, &tid);
    let ret = store.messages().find(&id.id());
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_local_message_query() {
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
        timestamp: 0,
    };

    store.messages().create(&msg).expect("create message");

    let id = utils::Id::new(&pid, &tid);
    let q = Query::new().push(Cond::and().push(Expr::eq("id", id.id())));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_local_message_update() {
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
        timestamp: 0,
    };

    store.messages().create(&msg).unwrap();

    let id = utils::Id::new(&pid, &tid);
    let mut msg = store.messages().find(&id.id()).unwrap();
    msg.state = "completed".to_string();
    msg.retry_times = 1;
    msg.status = MessageStatus::Acked;
    store.messages().update(&msg).unwrap();

    let msg2 = store.messages().find(&id.id()).unwrap();
    assert_eq!(msg2.state, "completed");
    assert_eq!(msg2.retry_times, 1);
    assert_eq!(msg2.status, MessageStatus::Acked);
}

#[tokio::test]
async fn store_local_message_remove() {
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
        timestamp: 0,
    };

    store.messages().create(&msg).unwrap();
    store.messages().delete(&msg.id).unwrap();

    let ret = store.messages().find(&msg.id);
    assert!(ret.is_err());
}

#[tokio::test]
async fn store_local_package_create() {
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
async fn store_local_package_query() {
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
async fn store_local_package_update() {
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
async fn store_local_package_remove() {
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
