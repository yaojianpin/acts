use crate::{
    MessageState, Query, StoreAdapter, TaskState, Workflow,
    data::Model,
    scheduler::NodeKind,
    store::{Cond, Store, StoreKind, data, query::Expr},
    utils,
};
use data::{Message, MessageStatus, Package, Proc, Task};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<Arc<Store>> = OnceCell::const_new();
async fn init() -> Arc<Store> {
    #[cfg(feature = "store")]
    {
        Arc::new(Store::local("test_data", "test.db"))
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
        store.procs().create(&proc).expect("create process");
    }

    let q = Query::new().set_limit(10000);
    let procs = store.procs().query(&q).unwrap();
    let procs = procs
        .rows
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
        store.procs().create(&proc).expect("create process");
    }

    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::Pending, &workflow);
        store.procs().create(&proc).expect("create process");
    }

    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow();
        let proc = create_proc(&id, TaskState::Completed, &workflow);
        store.procs().create(&proc).expect("create process");
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
        .rows
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
    assert!(ok);
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

    assert_eq!(models.rows.len(), 2);
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
async fn store_model_query_by_id() {
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
    store.models().create(&model).expect("create model");
    let q = Query::new().push(Cond::and().push(Expr::eq("id", model.id)));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_model_query_by_offset_count() {
    let store = store().await;
    let create_time = 100;
    for i in 0..10 {
        let model = Model {
            id: utils::longid(),
            name: format!("test-{}", i + 1),
            ver: 1,
            size: 1245,
            create_time,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: utils::time::timestamp(),
        };
        store.models().create(&model).expect("create model");
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("create_time", create_time)))
        .set_offset(0)
        .set_limit(5);
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 5);

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("create_time", create_time)))
        .set_offset(9)
        .set_limit(5);
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 1);
}

#[tokio::test]
async fn store_model_query_by_cond_and() {
    let store = store().await;
    let create_time = 200;
    for i in 0..10 {
        let model = Model {
            id: utils::longid(),
            name: format!("test-{}", i + 1),
            ver: 1,
            size: 1234,
            create_time,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: utils::time::timestamp(),
        };
        store.models().create(&model).expect("create model");
    }

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("create_time", create_time))
            .push(Expr::eq("size", 1234)),
    );
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 10);

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("create_time", create_time))
            .push(Expr::eq("size", 1000)),
    );
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 0);
}

#[tokio::test]
async fn store_model_query_by_cond_or() {
    let store = store().await;
    let create_time = 300;
    for i in 0..10 {
        let model = Model {
            id: utils::longid(),
            name: format!("test-{}", i + 1),
            ver: 1,
            size: 1234,
            create_time,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: utils::time::timestamp(),
        };
        store.models().create(&model).expect("create model");
    }
    for i in 0..10 {
        let model = Model {
            id: utils::longid(),
            name: format!("test-{}", i + 1),
            ver: 1,
            size: 2000,
            create_time,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: utils::time::timestamp(),
        };
        store.models().create(&model).expect("create model");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("create_time", create_time)))
        .push(
            Cond::or()
                .push(Expr::eq("size", 1234))
                .push(Expr::eq("size", 2000)),
        );
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.count, 20);
}

#[tokio::test]
async fn store_model_query_by_order() {
    let store = store().await;
    let create_time = 400;
    for i in 0..10 {
        let model = Model {
            id: utils::longid(),
            name: format!("test-{}", i + 1),
            ver: 1,
            size: 2000,
            create_time,
            update_time: 0,
            data: "{}".to_string(),
            timestamp: utils::time::timestamp(),
        };
        store.models().create(&model).expect("create model");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("create_time", create_time)))
        .push_order("timestamp", false);
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-10");

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("create_time", create_time)))
        .push_order("timestamp", true);
    let ret = store.models().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-1");
}

#[tokio::test]
async fn store_model_remove() {
    let store = store().await;

    let id = utils::longid();
    let mut workflow = create_workflow();
    workflow.id = id.clone();
    store.deploy(&workflow).unwrap();

    let model = store.models().find(&id);
    assert!(model.is_ok());

    store.models().delete(&id).unwrap();
    let model = store.models().find(&id);
    assert!(model.is_err());
}

#[tokio::test]
async fn store_model_deploy_id_error() {
    let store = store().await;
    let mut workflow = create_workflow();
    workflow.id = "".to_string();
    let result = store.deploy(&workflow);

    assert!(result.is_err());
}

#[tokio::test]
async fn store_proc_create() {
    let store = store().await;
    let id = utils::longid();
    let workflow = create_workflow();
    let proc = create_proc(&id, TaskState::None, &workflow);

    store.procs().create(&proc).expect("create process");

    let q = Query::new().set_limit(1);
    let procs = store.procs().query(&q).unwrap();
    assert_eq!(procs.rows.len(), 1);
}

#[tokio::test]
async fn store_proc_find() {
    let store = store().await;

    let id = utils::longid();
    let workflow = create_workflow();
    let proc = create_proc(&id, TaskState::None, &workflow);
    store.procs().create(&proc).expect("create process");
    let info = store.procs().find(&id).unwrap();
    assert_eq!(proc.id, info.id);
}

#[tokio::test]
async fn store_proc_query_by_id() {
    let store = store().await;

    let mid = utils::longid();
    let proc = Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: mid.clone(),
        state: "running".to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: utils::time::timestamp(),
        model: "{}".to_string(),
        env_local: "{}".to_string(),
        err: None,
    };

    store.procs().create(&proc).expect("create process");
    let q = Query::new().push(Cond::and().push(Expr::eq("id", proc.id)));
    let ret = store.procs().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_proc_query_by_offset_count() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..10 {
        let proc = Proc {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            mid: mid.clone(),
            state: "running".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: utils::time::timestamp(),
            model: "{}".to_string(),
            env_local: "{}".to_string(),
            err: None,
        };
        store.procs().create(&proc).expect("create process");
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("mid", mid.clone())))
        .set_offset(0)
        .set_limit(5);
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 5);

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("mid", mid.clone())))
        .set_offset(9)
        .set_limit(5);
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 1);
}

#[tokio::test]
async fn store_proc_query_by_cond_and() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..10 {
        let proc = Proc {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            mid: mid.clone(),
            state: "running".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: utils::time::timestamp(),
            model: "{}".to_string(),
            env_local: "{}".to_string(),
            err: None,
        };
        store.procs().create(&proc).expect("create process");
    }

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("mid", mid.clone()))
            .push(Expr::eq("state", "running")),
    );
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 10);

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("mid", mid.clone()))
            .push(Expr::eq("state", "created")),
    );
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 0);
}

#[tokio::test]
async fn store_proc_query_by_cond_or() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..10 {
        let proc = Proc {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            mid: mid.clone(),
            state: "running".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: utils::time::timestamp(),
            model: "{}".to_string(),
            env_local: "{}".to_string(),
            err: None,
        };
        store.procs().create(&proc).expect("create process");
    }

    for i in 0..10 {
        let proc = Proc {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            mid: mid.clone(),
            state: "completed".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: utils::time::timestamp(),
            model: "{}".to_string(),
            env_local: "{}".to_string(),
            err: None,
        };
        store.procs().create(&proc).expect("create process");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("mid", mid.clone())))
        .push(
            Cond::or()
                .push(Expr::eq("state", "running"))
                .push(Expr::eq("state", "completed")),
        );
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.count, 20);
}

#[tokio::test]
async fn store_proc_query_by_order() {
    let store = store().await;
    let mid = utils::longid();
    for i in 0..10 {
        let proc = Proc {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            mid: mid.clone(),
            state: "completed".to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: utils::time::timestamp(),
            model: "{}".to_string(),
            env_local: "{}".to_string(),
            err: None,
        };
        store.procs().create(&proc).expect("create process");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("mid", mid.clone())))
        .push_order("timestamp", false);
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-10");

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("mid", mid.clone())))
        .push_order("timestamp", true);
    let ret = store.procs().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-1");
}

#[tokio::test]
async fn store_proc_update() {
    let store = store().await;

    let id = utils::longid();
    let workflow = create_workflow();
    let mut proc = create_proc(&id, TaskState::None, &workflow);

    store.procs().create(&proc).expect("create process");

    proc.state = TaskState::Running.to_string();
    store.procs().update(&proc).expect("update process");

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

    store.procs().create(&proc).expect("create process");

    let proc = store.procs().find(&id);
    assert!(proc.is_ok());

    store.procs().delete(&id).unwrap();
    let proc = store.procs().find(&id);
    assert!(proc.is_err());
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
async fn store_task_query_by_id() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();
    let task = Task {
        id: format!("{pid}:{tid}"),
        name: "test".to_string(),
        prev: None,
        kind: NodeKind::Step.to_string(),
        pid: pid.clone(),
        tid: tid.clone(),
        node_data: "{}".to_string(),
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
    let q = Query::new().push(Cond::and().push(Expr::eq("id", id.id())));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_task_query_by_offset_count() {
    let store = store().await;
    let pid = utils::longid();
    for i in 0..10 {
        let tid = utils::shortid();
        let task = Task {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            prev: None,
            kind: NodeKind::Step.to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            node_data: "{}".to_string(),
            state: TaskState::None.to_string(),
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).expect("create task");
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .set_offset(0)
        .set_limit(5);
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 5);

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .set_offset(9)
        .set_limit(5);
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 1);
}

#[tokio::test]
async fn store_task_query_by_cond_and() {
    let store = store().await;
    let pid = utils::longid();
    for i in 0..10 {
        let tid = utils::shortid();
        let task = Task {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            prev: None,
            kind: NodeKind::Step.to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            node_data: "{}".to_string(),
            state: TaskState::None.to_string(),
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).expect("create task");
    }

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("pid", pid.clone()))
            .push(Expr::eq("state", "none")),
    );
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 10);

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("pid", pid.clone()))
            .push(Expr::eq("state", "created")),
    );
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 0);
}

#[tokio::test]
async fn store_task_query_by_cond_or() {
    let store = store().await;
    let pid = utils::longid();
    for i in 0..10 {
        let tid = utils::shortid();
        let task = Task {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            prev: None,
            kind: NodeKind::Step.to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            node_data: "{}".to_string(),
            state: TaskState::None.to_string(),
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).expect("create task");
    }

    for i in 0..10 {
        let tid = utils::shortid();
        let task = Task {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            prev: None,
            kind: NodeKind::Step.to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            node_data: "{}".to_string(),
            state: TaskState::Interrupt.to_string(),
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: 0,
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).expect("create task");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push(
            Cond::or()
                .push(Expr::eq("state", TaskState::Interrupt.to_string()))
                .push(Expr::eq("state", TaskState::None.to_string())),
        );
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.count, 20);
}

#[tokio::test]
async fn store_task_query_by_order() {
    let store = store().await;
    let pid = utils::longid();
    for i in 0..10 {
        let tid = utils::shortid();
        let task = Task {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
            prev: None,
            kind: NodeKind::Step.to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            node_data: "{}".to_string(),
            state: TaskState::None.to_string(),
            start_time: 0,
            end_time: 0,
            hooks: "{}".to_string(),
            timestamp: utils::time::timestamp(),
            data: "{}".to_string(),
            err: None,
        };
        store.tasks().create(&task).expect("create task");
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push_order("timestamp", false);
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-10");

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push_order("timestamp", true);
    let ret = store.tasks().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-1");
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
    store.tasks().delete(&task.id).expect("remove process");

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
async fn store_message_query_by_id() {
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
    let q = Query::new().push(Cond::and().push(Expr::eq("id", id.id())));
    let ret = store.messages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_message_query_by_offset_count() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();

    for _ in 0..100 {
        let msg = Message {
            id: utils::shortid(),
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
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(10)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())));
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 100);
    assert_eq!(ret.rows.len(), 10);

    let q = Query::new()
        .set_offset(95)
        .set_limit(10)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())));
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 100);
    assert_eq!(ret.rows.len(), 5);
}

#[tokio::test]
async fn store_message_query_by_cond_and() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();

    for _ in 0..100 {
        let msg = Message {
            id: utils::shortid(),
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
    }

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("pid", pid.clone()))
            .push(Expr::eq("type", "step")),
    );
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 100);

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("pid", pid.clone()))
            .push(Expr::eq("type", "workflow")),
    );
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 0);
}

#[tokio::test]
async fn store_message_query_by_cond_or() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();

    for _ in 0..10 {
        let msg = Message {
            id: utils::shortid(),
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
    }

    for _ in 0..10 {
        let msg = Message {
            id: utils::shortid(),
            name: "test".to_string(),
            pid: pid.clone(),
            tid: tid.clone(),
            nid: utils::shortid(),
            mid: utils::shortid(),
            state: MessageState::Completed,
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
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push(
            Cond::or()
                .push(Expr::eq("state", "created"))
                .push(Expr::eq("state", "completed")),
        );
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.count, 20);
}

#[tokio::test]
async fn store_message_query_by_order() {
    let store = store().await;

    let pid = utils::longid();
    let tid = utils::shortid();

    for i in 0..100 {
        let msg = Message {
            id: utils::shortid(),
            name: format!("test-{}", i + 1),
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
            timestamp: utils::time::timestamp(),
            status: MessageStatus::Created,
        };
        store.messages().create(&msg).unwrap();
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push_order("timestamp", false);
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-100");

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("pid", pid.clone())))
        .push_order("timestamp", true);
    let ret = store.messages().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().name, "test-1");
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
    msg.status = MessageStatus::Completed;
    store.messages().update(&msg).unwrap();

    let msg2 = store.messages().find(&id.id()).unwrap();
    assert_eq!(msg2.state, MessageState::Completed);
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
        run_as: crate::ActRunAs::Func,
        groups: "[]".to_string(),
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
async fn store_package_query_by_id() {
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
        groups: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
    };
    store.packages().create(&package).unwrap();
    let q = Query::new().push(Cond::and().push(Expr::eq("id", package.id)));
    let ret = store.packages().query(&q);
    assert!(ret.is_ok());
}

#[tokio::test]
async fn store_package_query_by_offset_count() {
    let store = store().await;
    for _i in 0..10 {
        let package = Package {
            id: utils::shortid(),
            desc: "desc".to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 100,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("create_time", 100)))
        .set_offset(0)
        .set_limit(5);
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 5);

    let q = Query::new()
        .push(Cond::and().push(Expr::eq("create_time", 100)))
        .set_offset(9)
        .set_limit(5);
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 10);
    assert_eq!(ret.rows.len(), 1);
}

#[tokio::test]
async fn store_package_query_by_cond_and() {
    let store = store().await;
    for _i in 0..10 {
        let package = Package {
            id: utils::shortid(),
            desc: "desc".to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 200,
            update_time: 100,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("built_in", false))
            .push(Expr::eq("create_time", 200))
            .push(Expr::eq("update_time", 100)),
    );
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 10);

    let q = Query::new().set_offset(0).set_limit(10).push(
        Cond::and()
            .push(Expr::eq("built_in", false))
            .push(Expr::eq("create_time", 200))
            .push(Expr::eq("update_time", 200)),
    );
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 0);
}

#[tokio::test]
async fn store_package_query_by_cond_or() {
    let store = store().await;
    for _i in 0..10 {
        let package = Package {
            id: utils::shortid(),
            desc: "desc".to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 300,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    for _i in 0..10 {
        let package = Package {
            id: utils::shortid(),
            desc: "desc".to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.2.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 300,
            update_time: 0,
            timestamp: 0,
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("create_time", 300)))
        .push(
            Cond::or()
                .push(Expr::eq("version", "0.1.0"))
                .push(Expr::eq("version", "0.2.0")),
        );
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.count, 20);
}

#[tokio::test]
async fn store_package_query_by_order() {
    let store = store().await;
    for i in 0..10 {
        let package = Package {
            id: utils::shortid(),
            desc: format!("test-{}", i + 1),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: 400,
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        store.packages().create(&package).unwrap();
    }

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(
            Cond::and()
                .push(Expr::eq("built_in", false))
                .push(Expr::eq("create_time", 400)),
        )
        .push_order("timestamp", false);
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().desc, "test-10");

    let q = Query::new()
        .set_offset(0)
        .set_limit(100)
        .push(Cond::and().push(Expr::eq("create_time", 400)))
        .push_order("timestamp", true);
    let ret = store.packages().query(&q).unwrap();
    assert_eq!(ret.rows.last().unwrap().desc, "test-1");
}

#[tokio::test]
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
        run_as: crate::ActRunAs::Func,
        groups: "[]".to_string(),
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
    store.packages().update(&p).unwrap();

    let p2 = store.packages().find(&package.id).unwrap();
    assert_eq!(p2.desc, "my desc");
    assert_eq!(p2.version, "0.2.0");
}

#[tokio::test]
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
        run_as: crate::ActRunAs::Func,
        groups: "[]".to_string(),
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
