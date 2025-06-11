use crate::{
    MessageState, TaskState, Vars,
    scheduler::NodeKind,
    store::{Filter, Query, data::*, db::MemStore, query::Expr},
    utils,
};
use serde_json::json;
use tokio::sync::OnceCell;

static STORE: OnceCell<MemStore> = OnceCell::const_new();
async fn init() -> MemStore {
    MemStore::new()
}

async fn store() -> &'static MemStore {
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
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        timestamp: 0,
    };
    store.models().create(&model).unwrap();
    assert!(store.models().exists(&model.id).unwrap());
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
        create_time: 3333,
        data: "{}".to_string(),
        update_time: 0,
        timestamp: 0,
    };
    store.models().create(&model).unwrap();
    assert_eq!(store.models().find(&mid).unwrap().id, mid);
}

#[tokio::test]
async fn store_mem_model_query_id() {
    let store = store().await;
    let models = store.models();
    for _ in 0..5 {
        let model = Model {
            id: utils::longid(),
            name: "test_model".to_string(),
            ver: 1,
            size: 1245,
            create_time: 3333,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: 0,
        };
        models.create(&model).unwrap();
    }

    let q = Query::new()
        .filter(Filter::and().expr(Expr::eq("name", "test_model")))
        .limit(5);
    let items = models.query(&q).unwrap();
    assert_eq!(items.count, 5);
}

#[tokio::test]
async fn store_mem_model_query_match_or() {
    let store = store().await;
    for i in 0..5 {
        let model = Model {
            id: utils::longid(),
            name: format!("test_model {i}"),
            ver: 1,
            size: 1000,
            create_time: 3333,
            update_time: 0,
            data: format!("data {i}"),
            timestamp: 0,
        };
        store.models().create(&model).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("size", 1000)).push(
            Filter::or()
                .expr(Expr::matches("name", "test_model"))
                .expr(Expr::matches("data", "data")),
        ),
    );

    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_model_query_match_and() {
    let store = store().await;
    for i in 0..5 {
        let model = Model {
            id: utils::longid(),
            name: format!("test_model {i}"),
            ver: 1,
            size: 2000,
            create_time: 3333,
            update_time: 0,
            data: format!("data {i}"),
            timestamp: 0,
        };
        store.models().create(&model).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("size", 2000)).push(
            Filter::and()
                .expr(Expr::matches("name", "0"))
                .expr(Expr::matches("data", "0")),
        ),
    );

    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 1);
}

#[tokio::test]
async fn store_mem_model_update() {
    let store = store().await;

    let mut model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        timestamp: 0,
    };
    store.models().create(&model).unwrap();

    model.ver = 3;
    model.update_time = 1;
    store.models().update(&model).unwrap();

    let p = store.models().find(&model.id).unwrap();
    assert_eq!(p.ver, model.ver);
    assert!(p.update_time > 0);
}

#[tokio::test]
async fn store_mem_model_delete() {
    let store = store().await;
    let model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        timestamp: 0,
    };
    store.models().create(&model).unwrap();
    store.models().delete(&model.id).unwrap();

    assert!(!store.procs().exists(&model.id).unwrap());
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
        env: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert!(store.procs().exists(&proc.id).unwrap());
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
        env: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().find(&pid).unwrap().id, pid);
}

#[tokio::test]
async fn store_mem_proc_query_id() {
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
            env: "".to_string(),
            err: None,
        };
        procs.create(&proc).unwrap();
    }

    let q = Query::new()
        .filter(Filter::and().expr(Expr::eq("mid", mid)))
        .limit(5);
    let items = procs.query(&q).unwrap();
    assert_eq!(items.count, 5);
}

#[tokio::test]
async fn store_mem_proc_query_match_or() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: format!("name {i}"),
            mid: mid.to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: format!("model {i}"),
            env: "".to_string(),
            err: None,
        };
        store.procs().create(&proc).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("mid", mid)).push(
            Filter::or()
                .expr(Expr::matches("name", "name"))
                .expr(Expr::matches("model", "model")),
        ),
    );

    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_proc_query_match_and() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: format!("name {i}"),
            mid: mid.to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: format!("model {i}"),
            env: "".to_string(),
            err: None,
        };
        store.procs().create(&proc).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("mid", mid)).push(
            Filter::and()
                .expr(Expr::matches("name", "0"))
                .expr(Expr::matches("model", "0")),
        ),
    );

    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 1);
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
        env: "".to_string(),
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
        env: "".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    store.procs().delete(&proc.id).unwrap();

    assert!(!store.procs().exists(&proc.id).unwrap());
}

#[tokio::test]
async fn store_mem_task_create() {
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
    assert!(tasks.exists(&task.id).unwrap());
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
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
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
async fn store_mem_task_query_id() {
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
        .filter(Filter::and().expr(Expr::eq("pid", pid)))
        .limit(5);
    let items = tasks.query(&q).unwrap();
    assert_eq!(items.count, 5);
}

#[tokio::test]
async fn store_mem_task_query_match_or() {
    let store = store().await;
    let pid = utils::shortid();
    for idx in 0..5 {
        let task = Task {
            kind: NodeKind::Workflow.into(),
            id: utils::shortid(),
            name: format!("test {idx}"),
            tid: format!("tid {idx}"),
            pid: pid.to_string(),
            node_data: "nid2".to_string(),
            state: TaskState::None.into(),
            prev: None,
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("node_data", "nid2")).push(
            Filter::or()
                .expr(Expr::matches("name", "test"))
                .expr(Expr::matches("tid", "tid")),
        ),
    );

    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_task_query_match_and() {
    let store = store().await;

    let pid = utils::shortid();
    for idx in 0..5 {
        let task = Task {
            kind: NodeKind::Workflow.into(),
            id: utils::shortid(),
            name: format!("test {idx}"),
            tid: format!("tid {idx}"),
            pid: pid.to_string(),
            node_data: "nid3".to_string(),
            state: TaskState::None.into(),
            prev: None,
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("node_data", "nid3")).push(
            Filter::and()
                .expr(Expr::matches("name", "0"))
                .expr(Expr::matches("tid", "0")),
        ),
    );

    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 1);
}

#[tokio::test]
async fn store_mem_task_update() {
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
async fn store_mem_task_delete() {
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

    assert!(!table.exists(&task.id).unwrap());
}

#[tokio::test]
async fn store_mem_message_create() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        uses: "package".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        timestamp: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).expect("create message");

    let id = utils::Id::new(&pid, &tid);
    let ret = store.messages().find(&id.id());
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_mem_message_query_id() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        uses: "package".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        timestamp: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).expect("create message");

    let id = utils::Id::new(&pid, &tid);
    let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_mem_message_query_match_or() {
    let store = store().await;

    for idx in 0..5 {
        let pid = utils::longid();
        let tid = utils::shortid();
        let msg = Message {
            id: format!("{pid}:{tid}"),
            name: "test".to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            nid: utils::shortid(),
            mid: utils::shortid(),
            state: MessageState::Created,
            start_time: 0,
            end_time: 0,
            r#type: "step".to_string(),
            model: json!({ "id": "m1"}).to_string(),
            key: format!("test {idx}"),
            uses: format!("package {idx}"),
            inputs: json!({}).to_string(),
            outputs: json!({}).to_string(),
            tag: "tag1".to_string(),
            chan_id: "test2".to_string(),
            chan_pattern: "*:*:*:*".to_string(),
            create_time: 0,
            update_time: 0,
            retry_times: 0,
            timestamp: 0,
            status: MessageStatus::Created,
        };

        store.messages().create(&msg).expect("create message");
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("chan_id", "test2")).push(
            Filter::or()
                .expr(Expr::matches("key", "test"))
                .expr(Expr::matches("uses", "package")),
        ),
    );

    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_message_query_match_and() {
    let store = store().await;

    for idx in 0..5 {
        let pid = utils::longid();
        let tid = utils::shortid();
        let msg = Message {
            id: format!("{pid}:{tid}"),
            name: "test".to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            nid: utils::shortid(),
            mid: utils::shortid(),
            state: MessageState::Created,
            start_time: 0,
            end_time: 0,
            r#type: "step".to_string(),
            model: json!({ "id": "m1"}).to_string(),
            key: format!("test {idx}"),
            uses: format!("package {idx}"),
            inputs: json!({}).to_string(),
            outputs: json!({}).to_string(),
            tag: "tag1".to_string(),
            chan_id: "test3".to_string(),
            chan_pattern: "*:*:*:*".to_string(),
            create_time: 0,
            update_time: 0,
            retry_times: 0,
            timestamp: 0,
            status: MessageStatus::Created,
        };

        store.messages().create(&msg).expect("create message");
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("chan_id", "test3")).push(
            Filter::and()
                .expr(Expr::matches("key", "0"))
                .expr(Expr::matches("uses", "0")),
        ),
    );

    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 1);
}

#[tokio::test]
async fn store_mem_message_update() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        uses: "package".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        timestamp: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).unwrap();

    let id = utils::Id::new(&pid, &tid);
    let mut msg = store.messages().find(&id.id()).unwrap();
    msg.state = MessageState::Completed;
    msg.retry_times = 1;
    msg.status = MessageStatus::Acked;
    store.messages().update(&msg).unwrap();

    let msg2 = store.messages().find(&id.id()).unwrap();
    assert_eq!(msg2.state, MessageState::Completed);
    assert_eq!(msg2.retry_times, 1);
    assert_eq!(msg2.status, MessageStatus::Acked);
}

#[tokio::test]
async fn store_mem_message_remove() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let msg = Message {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        uses: "package".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: 0,
        update_time: 0,
        retry_times: 0,
        timestamp: 0,
        status: MessageStatus::Created,
    };

    store.messages().create(&msg).unwrap();
    store.messages().delete(&msg.id).unwrap();

    let ret = store.messages().find(&msg.id);
    assert!(ret.is_err());
}

#[tokio::test]
async fn store_mem_package_create() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };

    store.packages().create(&package).unwrap();
    let ret = store.packages().find(&package.id);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_mem_package_query_id() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };
    store.packages().create(&package).unwrap();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("id", package.id)));
    let ret = store.packages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_mem_package_query_match_or() {
    let store = store().await;

    for idx in 0..5 {
        let id = utils::longid();
        let package = Package {
            id,
            desc: format!("desc text {idx}"),
            icon: format!("icon text {idx}"),
            doc: "doc".to_string(),
            version: "0.2.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            resources: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 0,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("version", "0.2.0")).push(
            Filter::or()
                .expr(Expr::matches("desc", "desc"))
                .expr(Expr::matches("icon", "icon")),
        ),
    );

    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_package_query_match_and() {
    let store = store().await;

    for idx in 0..5 {
        let id = utils::longid();
        let package = Package {
            id,
            desc: format!("desc text {idx}"),
            icon: format!("icon text {idx}"),
            doc: "doc".to_string(),
            version: "0.3.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            resources: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 0,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("version", "0.3.0")).push(
            Filter::and()
                .expr(Expr::matches("desc", "0"))
                .expr(Expr::matches("icon", "0")),
        ),
    );

    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 1);
}

#[tokio::test]
async fn store_mem_package_update() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };
    store.packages().create(&package).unwrap();
    let mut p = store.packages().find(&package.id).unwrap();
    p.desc = "my desc".to_string();
    p.version = "0.2.0".to_string();
    p.schema = "{ 'b': 100 }".to_string();
    store.packages().update(&p).unwrap();

    let p2 = store.packages().find(&package.id).unwrap();
    assert_eq!(p2.desc, "my desc");
    assert_eq!(p2.version, "0.2.0");
    assert_eq!(p2.schema, "{ 'b': 100 }");
}

#[tokio::test]
async fn store_mem_package_remove() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };
    store.packages().create(&package).unwrap();
    store.packages().delete(&package.id).unwrap();

    let ret = store.packages().find(&package.id);
    assert!(ret.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_mem_event_create() {
    let store = store().await;

    let id = utils::longid();
    let evt = Event {
        id,
        name: "name".to_string(),
        mid: "mid".to_string(),
        ver: 1,
        uses: "acts.event.manual".to_string(),
        params: "".to_string(),
        create_time: utils::time::time_millis(),
        timestamp: utils::time::timestamp(),
    };

    store.events().create(&evt).unwrap();
    let ret = store.events().find(&evt.id);
    assert!(ret.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_mem_event_query_id() {
    let store = store().await;

    let id = utils::longid();
    let evt = Event {
        id,
        name: "name".to_string(),
        mid: "mid".to_string(),
        ver: 1,
        uses: "acts.event.manual".to_string(),
        params: "".to_string(),
        create_time: 0,
        timestamp: 0,
    };
    store.events().create(&evt).unwrap();
    let q = Query::new().filter(Filter::and().expr(Expr::eq("id", evt.id)));
    let ret = store.events().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_mem_event_query_match_or() {
    let store = store().await;

    for idx in 0..5 {
        let id = utils::longid();
        let evt = Event {
            id,
            name: format!("name {idx}"),
            mid: "mid1".to_string(),
            ver: 1,
            uses: "acts.event.manual".to_string(),
            params: "".to_string(),
            create_time: 0,
            timestamp: 0,
        };
        store.events().create(&evt).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("mid", "mid1")).push(
            Filter::or()
                .expr(Expr::matches("name", "name"))
                .expr(Expr::matches("uses", "manual")),
        ),
    );

    let ret = store.events().query(&q).unwrap();
    assert_eq!(ret.count, 5);
}

#[tokio::test]
async fn store_mem_event_query_match_and() {
    let store = store().await;

    for idx in 0..5 {
        let id = utils::longid();
        let evt = Event {
            id,
            name: format!("name {idx}"),
            mid: "mid2".to_string(),
            ver: 1,
            uses: "acts.event.manual".to_string(),
            params: "".to_string(),
            create_time: 0,
            timestamp: 0,
        };
        store.events().create(&evt).unwrap();
    }

    let q = Query::new().filter(
        Filter::and().expr(Expr::eq("mid", "mid2")).push(
            Filter::and()
                .expr(Expr::matches("name", "0"))
                .expr(Expr::matches("uses", "manual")),
        ),
    );

    let ret = store.events().query(&q).unwrap();
    assert_eq!(ret.count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_mem_event_update() {
    let store = store().await;

    let id = utils::longid();
    let evt = Event {
        id,
        name: "name".to_string(),
        mid: "mid".to_string(),
        ver: 1,
        uses: "acts.event.manual".to_string(),
        params: "".to_string(),
        create_time: 0,
        timestamp: 0,
    };
    store.events().create(&evt).unwrap();
    let mut p = store.events().find(&evt.id).unwrap();
    p.name = "my name".to_string();
    p.timestamp = 200;
    p.mid = "my mid".to_string();

    store.events().update(&p).unwrap();

    let p2 = store.events().find(&evt.id).unwrap();
    assert_eq!(p2.name, "my name");
    assert_eq!(p2.timestamp, 200);
    assert_eq!(p2.mid, "my mid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_mem_event_remove() {
    let store = store().await;

    let id = utils::longid();
    let evt = Event {
        id,
        name: "name".to_string(),
        mid: "mid".to_string(),
        ver: 1,
        uses: "acts.event.manual".to_string(),
        params: "".to_string(),
        create_time: 0,
        timestamp: 0,
    };
    store.events().create(&evt).unwrap();
    store.events().delete(&evt.id).unwrap();

    let ret = store.events().find(&evt.id);
    assert!(ret.is_err());
}
