use crate::database::Database;
use acts::{MessageState, Vars, data::*, query::*};
use serde_json::json;
use tokio::sync::OnceCell;

static STORE: OnceCell<Database> = OnceCell::const_new();
async fn init() -> Database {
    let db = Database::new("sqlite://test_data/test.db");
    db.init();
    db
}

async fn store() -> &'static Database {
    STORE.get_or_init(init).await
}
mod utils {
    use nanoid::nanoid;
    pub fn longid() -> String {
        nanoid!(21)
    }

    pub fn shortid() -> String {
        nanoid!(8)
    }

    pub fn time_millis() -> i64 {
        let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
        time.timestamp_millis()
    }

    pub fn timestamp() -> i64 {
        let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
        time.timestamp_micros()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_model_create() {
    let store = store().await;
    let model = Model {
        id: utils::longid(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        create_time: utils::time_millis(),
        update_time: 0,
        data: "{}".to_string(),
        timestamp: utils::timestamp(),
    };
    store.models().create(&model).unwrap();
    assert!(store.models().exists(&model.id).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_model_find() {
    let store = store().await;
    let mid: String = utils::longid();
    let model = Model {
        id: mid.clone(),
        name: "test".to_string(),
        ver: 1,
        size: 1245,
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        timestamp: 0,
    };
    store.models().create(&model).unwrap();
    assert_eq!(store.models().find(&mid).unwrap().id, mid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_model_query_id() {
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
    assert_eq!(items.rows.len(), 5);
}

#[tokio::test]
async fn store_model_query_match_or() {
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
async fn store_model_query_match_and() {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_model_update() {
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
    model.update_time = utils::time_millis();
    store.models().update(&model).unwrap();

    let p = store.models().find(&model.id).unwrap();
    assert_eq!(p.ver, model.ver);
    assert!(p.update_time > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_model_delete() {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_proc_create() {
    let store = store().await;
    let proc = Proc {
        id: utils::longid(),
        name: "name".to_string(),
        mid: "m1".to_string(),
        state: "none".to_string(),
        start_time: utils::time_millis(),
        end_time: 0,
        timestamp: utils::timestamp(),
        model: "".to_string(),
        env: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert!(store.procs().exists(&proc.id).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_proc_find() {
    let store = store().await;
    let pid = utils::longid();
    let proc = Proc {
        id: pid.clone(),
        name: "name".to_string(),
        mid: "m1".to_string(),
        state: "none".to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        env: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    assert_eq!(store.procs().find(&pid).unwrap().id, pid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_proc_query_id() {
    let store = store().await;
    let procs = store.procs();
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: i.to_string(),
            mid: mid.to_string(),
            state: "none".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: "".to_string(),
            env: "{}".to_string(),
            err: None,
        };
        procs.create(&proc).unwrap();
    }

    let q = Query::new()
        .filter(Filter::and().expr(Expr::eq("mid", mid)))
        .limit(5);
    let items = procs.query(&q).unwrap();
    assert_eq!(items.rows.len(), 5);
}

#[tokio::test]
async fn store_proc_query_match_or() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: format!("name {i}"),
            mid: mid.to_string(),
            state: "none".to_string(),
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
async fn store_proc_query_match_and() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..5 {
        let proc = Proc {
            id: utils::longid(),
            name: format!("name {i}"),
            mid: mid.to_string(),
            state: "none".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_proc_update() {
    let store = store().await;

    let mut vars: Vars = Vars::new();
    vars.insert("k1".to_string(), "v1".into());

    let mut proc = Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: "none".to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: "".to_string(),
        env: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();

    proc.state = "running".to_string();
    proc.err = None;
    proc.end_time = utils::time_millis();
    store.procs().update(&proc).unwrap();

    let p = store.procs().find(&proc.id).unwrap();
    assert_eq!(p.state, proc.state);
    assert_eq!(p.err, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_proc_delete() {
    let store = store().await;
    let proc = Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: "none".to_string(),
        start_time: utils::time_millis(),
        end_time: 0,
        timestamp: utils::timestamp(),
        model: "".to_string(),
        env: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).unwrap();
    store.procs().delete(&proc.id).unwrap();

    assert!(!store.procs().exists(&proc.id).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_task_create() {
    let store = store().await;
    let tasks = store.tasks();
    let task = Task {
        id: utils::shortid(),
        kind: "workflow".to_string(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
        state: "none".to_string(),
        prev: None,
        start_time: utils::time_millis(),
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: utils::timestamp(),
        data: "{}".to_string(),
        err: None,
    };
    tasks.create(&task).unwrap();
    assert!(tasks.exists(&task.id).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_task_find() {
    let store = store().await;
    let tasks = store.tasks();
    let tid = utils::shortid();
    let task = Task {
        id: tid.clone(),
        kind: "workflow".to_string(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
        state: "none".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_task_query_id() {
    let store = store().await;
    let tasks = store.tasks();
    let pid = utils::shortid();
    for _ in 0..5 {
        let task = Task {
            kind: "workflow".to_string(),
            id: utils::shortid(),
            name: "test".to_string(),
            pid: pid.to_string(),
            tid: "tid".to_string(),
            node_data: "nid".to_string(),
            state: "none".to_string(),
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
    assert_eq!(items.rows.len(), 5);
}

#[tokio::test]
async fn store_task_query_match_or() {
    let store = store().await;
    let pid = utils::shortid();
    for idx in 0..5 {
        let task = Task {
            kind: "workflow".to_string(),
            id: utils::shortid(),
            name: format!("test {idx}"),
            tid: format!("tid {idx}"),
            pid: pid.to_string(),
            node_data: "nid2".to_string(),
            state: "none".to_string(),
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
async fn store_task_query_match_and() {
    let store = store().await;

    let pid = utils::shortid();
    for idx in 0..5 {
        let task = Task {
            kind: "workflow".to_string(),
            id: utils::shortid(),
            name: format!("test {idx}"),
            tid: format!("tid {idx}"),
            pid: pid.to_string(),
            node_data: "nid3".to_string(),
            state: "none".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_task_update() {
    let store = store().await;
    let table = store.tasks();
    let mut task = Task {
        kind: "workflow".to_string(),
        id: utils::shortid(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
        state: "none".to_string(),
        prev: None,
        start_time: 0,
        end_time: 0,
        hooks: "{}".to_string(),
        timestamp: 0,
        data: "{}".to_string(),
        err: None,
    };
    table.create(&task).unwrap();

    task.state = "completed".to_string();
    task.prev = Some("tid1".to_string());
    task.end_time = utils::time_millis();
    table.update(&task).unwrap();

    let t = table.find(&task.id).unwrap();
    assert_eq!(t.state, task.state);
    assert_eq!(t.prev, task.prev);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_task_delete() {
    let store = store().await;
    let table = store.tasks();
    let task = Task {
        kind: "workflow".to_string(),
        id: utils::shortid(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: "nid".to_string(),
        state: "none".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_message_create() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();

    let id = format!("{pid}:{tid}");
    let msg: Message = Message {
        id: id.clone(),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        uses: "pack1".to_string(),
        r#type: "step".to_string(),
        model: json!({ "id": "m1"}).to_string(),
        key: "test".to_string(),
        inputs: json!({}).to_string(),
        outputs: json!({}).to_string(),
        tag: "tag1".to_string(),
        chan_id: "test1".to_string(),
        chan_pattern: "*:*:*:*".to_string(),
        create_time: utils::time_millis(),
        update_time: 0,
        retry_times: 0,
        status: MessageStatus::Created,
        timestamp: utils::timestamp(),
    };

    store.messages().create(&msg).expect("create message");

    let ret = store.messages().find(&id);
    assert!(ret.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_message_query_id() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let id = format!("{pid}:{tid}");
    let msg = Message {
        id: id.clone(),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        uses: "pack1".to_string(),
        r#type: "step".to_string(),
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

    let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id)));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_message_update() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let id = format!("{pid}:{tid}");
    let msg = Message {
        id: id.clone(),
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        uses: "pack1".to_string(),
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

    let mut msg = store.messages().find(&id).unwrap();
    msg.state = MessageState::Completed;
    msg.retry_times = 1;
    msg.status = MessageStatus::Acked;
    msg.update_time = utils::time_millis();
    store.messages().update(&msg).unwrap();

    let msg2 = store.messages().find(&id).unwrap();
    assert_eq!(msg2.state, MessageState::Completed);
    assert_eq!(msg2.retry_times, 1);
    assert_eq!(msg2.status, MessageStatus::Acked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_message_remove() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let id = format!("{pid}:{tid}");
    let msg = Message {
        id,
        name: "test".to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        nid: utils::shortid(),
        mid: utils::shortid(),
        state: MessageState::Created,
        start_time: 0,
        end_time: 0,
        r#type: "step".to_string(),
        uses: "pack1".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_package_create() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: acts::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: acts::ActPackageCatalog::Core,
        create_time: utils::time_millis(),
        update_time: 0,
        timestamp: utils::timestamp(),
        built_in: false,
    };

    store.packages().create(&package).unwrap();
    let ret = store.packages().find(&package.id);
    assert!(ret.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_package_query_id() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: acts::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: acts::ActPackageCatalog::Core,
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
async fn store_package_query_match_or() {
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
            run_as: acts::ActRunAs::Func,
            resources: "[]".to_string(),
            catalog: acts::ActPackageCatalog::Core,
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
async fn store_package_query_match_and() {
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
            run_as: acts::ActRunAs::Func,
            resources: "[]".to_string(),
            catalog: acts::ActPackageCatalog::Core,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_package_update() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: acts::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: acts::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };
    store.packages().create(&package).unwrap();
    let mut p = store.packages().find(&package.id).unwrap();
    p.desc = "my name".to_string();
    p.update_time = utils::time_millis();
    p.schema = "{\"a\": 0 }".to_string();
    store.packages().update(&p).unwrap();

    let p2 = store.packages().find(&package.id).unwrap();
    assert_eq!(p2.desc, "my name");
    assert_eq!(p2.update_time, p.update_time);
    assert_eq!(p2.schema, "{\"a\": 0 }");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_package_remove() {
    let store = store().await;

    let id = utils::longid();
    let package = Package {
        id,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: acts::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: acts::ActPackageCatalog::Core,
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
async fn store_event_create() {
    let store = store().await;

    let id = utils::longid();
    let evt = Event {
        id,
        name: "name".to_string(),
        mid: "mid".to_string(),
        ver: 1,
        uses: "acts.event.manual".to_string(),
        params: "".to_string(),
        create_time: utils::time_millis(),
        timestamp: utils::timestamp(),
    };

    store.events().create(&evt).unwrap();
    let ret = store.events().find(&evt.id);
    assert!(ret.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn store_event_query_id() {
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
async fn store_event_query_match_or() {
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
async fn store_event_query_match_and() {
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
async fn store_event_update() {
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
async fn store_event_remove() {
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
