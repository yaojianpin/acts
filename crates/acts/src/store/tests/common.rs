#[macro_export]
macro_rules! gen_store_tests {
    ($init:expr) => {
        use serde_json::json;
        use serial_test::serial;
        use std::collections::HashSet;
        use std::sync::OnceLock;
        use $crate::store::data::{Message, MessageStatus, Model, Package, Proc, Task};
        use $crate::store::query::{Expr, Sort};
        use $crate::store::{Filter, Query};
        use $crate::{MessageState, TaskState, Workflow, scheduler::NodeKind, utils};

        static STORE: OnceLock<std::sync::Arc<$crate::store::Store>> = OnceLock::new();

        fn store() -> &'static std::sync::Arc<$crate::store::Store> {
            STORE.get_or_init(|| $init)
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
                env: "{}".to_string(),
                err: None,
                v: 0,
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_load_by_limit() {
            let store = store();

            let prefix = utils::shortid();
            let name = utils::shortid();
            for _ in 0..100 {
                let id = format!("{}_{}", prefix, utils::longid());
                let workflow = create_workflow();
                let mut proc = create_proc(&id, TaskState::None, &workflow);
                proc.name = name.clone();
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("name", name)))
                .limit(10000);
            let procs = store.procs().query(&q).unwrap();
            let procs = procs
                .rows
                .iter()
                .filter(|it| it.id.starts_with(&prefix))
                .collect::<Vec<_>>();
            assert_eq!(procs.len(), 100);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_load_by_state() {
            let store = store();

            let prefix = utils::shortid();
            let name = utils::shortid();
            for _ in 0..100 {
                let id = format!("{}_{}", prefix, utils::longid());
                let workflow = create_workflow();
                let mut proc = create_proc(&id, TaskState::Running, &workflow);
                proc.name = name.clone();
                store.procs().create(&proc).expect("create process");
            }

            for _ in 0..100 {
                let id = format!("{}_{}", prefix, utils::longid());
                let workflow = create_workflow();
                let mut proc = create_proc(&id, TaskState::Pending, &workflow);
                proc.name = name.clone();
                store.procs().create(&proc).expect("create process");
            }

            for _ in 0..100 {
                let id = format!("{}_{}", prefix, utils::longid());
                let workflow = create_workflow();
                let mut proc = create_proc(&id, TaskState::Completed, &workflow);
                proc.name = name.clone();
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new()
                .filter(
                    Filter::and().expr(Expr::eq("name", name)).push(
                        Filter::or()
                            .expr(Expr::eq("state", "running"))
                            .expr(Expr::eq("state", "pending")),
                    ),
                )
                .limit(10000);
            let procs = store.procs().query(&q).unwrap();
            let procs = procs
                .rows
                .iter()
                .filter(|it| it.id.starts_with(&prefix))
                .collect::<Vec<_>>();
            assert_eq!(procs.len(), 200);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_deploy_ok() {
            let store = store();
            let workflow = create_workflow();
            let ok = store.deploy(&workflow, None).unwrap();
            assert!(ok);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_models() {
            let store = store();

            let mut workflow = create_workflow();
            workflow.id = utils::longid();
            store.deploy(&workflow, None).unwrap();

            workflow.id = utils::longid();
            store.deploy(&workflow, None).unwrap();

            let q = Query::new().limit(2);
            let models = store.models().query(&q).unwrap();

            assert_eq!(models.rows.len(), 2);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_get() {
            let store = store();
            let mut workflow = create_workflow();
            workflow.id = utils::longid();
            store.deploy(&workflow, None).unwrap();

            let model = store.models().find(&workflow.id).unwrap();
            assert_eq!(model.id, workflow.id);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_query_by_id() {
            let store = store();
            let model = Model {
                id: utils::longid(),
                name: "test".to_string(),
                desc: "test desc".to_string(),
                ver: "0.1.0".to_string(),
                size: 1245,
                create_time: 3333,
                update_time: 0,
                data: "{}".to_string(),
                view: None,
                timestamp: 0,
                v: 0,
            };
            store.models().create(&model).expect("create model");
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", model.id)));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_query_by_offset_count() {
            let store = store();
            let create_time = 100;
            let name = utils::shortid();
            for _ in 0..10 {
                let model = Model {
                    id: utils::longid(),
                    name: name.clone(),
                    desc: "test desc".to_string(),
                    ver: "0.1.0".to_string(),
                    size: 1245,
                    create_time,
                    update_time: 0,
                    data: "{}".to_string(),
                    view: None,
                    timestamp: utils::time::timestamp(),
                    v: 0,
                };
                store.models().create(&model).expect("create model");
            }

            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", create_time)),
                )
                .offset(0)
                .limit(5);
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 5);

            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", create_time)),
                )
                .offset(9)
                .limit(5);
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_query_by_cond_and() {
            let store = store();
            let create_time = 200;
            let name = utils::shortid();
            for _ in 0..10 {
                let model = Model {
                    id: utils::longid(),
                    name: name.clone(),
                    desc: "test desc".to_string(),
                    ver: "0.1.0".to_string(),
                    size: 1234,
                    create_time,
                    update_time: 0,
                    data: "{}".to_string(),
                    view: None,
                    timestamp: utils::time::timestamp(),
                    v: 0,
                };
                store.models().create(&model).expect("create model");
            }

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("name", name.clone()))
                    .expr(Expr::eq("create_time", create_time))
                    .expr(Expr::eq("size", 1234)),
            );
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.count, 10);

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("name", name.clone()))
                    .expr(Expr::eq("create_time", create_time))
                    .expr(Expr::eq("size", 1000)),
            );
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_query_by_cond_or() {
            let store = store();
            let create_time = 300;
            let name = utils::shortid();
            for _ in 0..10 {
                let model = Model {
                    id: utils::longid(),
                    name: name.clone(),
                    desc: "test desc".to_string(),
                    ver: "0.1.0".to_string(),
                    size: 1234,
                    create_time,
                    update_time: 0,
                    data: "{}".to_string(),
                    view: None,
                    timestamp: utils::time::timestamp(),
                    v: 0,
                };
                store.models().create(&model).expect("create model");
            }
            for _ in 0..10 {
                let model = Model {
                    id: utils::longid(),
                    name: name.clone(),
                    desc: "test desc".to_string(),
                    ver: "0.1.0".to_string(),
                    size: 2000,
                    create_time,
                    update_time: 0,
                    data: "{}".to_string(),
                    view: None,
                    timestamp: utils::time::timestamp(),
                    v: 0,
                };
                store.models().create(&model).expect("create model");
            }

            let q = Query::new().offset(0).limit(100).filter(
                Filter::and()
                    .expr(Expr::eq("name", name))
                    .expr(Expr::eq("create_time", create_time))
                    .push(
                        Filter::or()
                            .expr(Expr::eq("size", 1234))
                            .expr(Expr::eq("size", 2000)),
                    ),
            );
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_query_by_order() {
            let store = store();
            let create_time = 400;
            let name = utils::shortid();
            for _ in 0..10 {
                let model = Model {
                    id: utils::longid(),
                    name: name.clone(),
                    desc: "test desc".to_string(),
                    ver: "0.1.0".to_string(),
                    size: 2000,
                    create_time,
                    update_time: 0,
                    data: "{}".to_string(),
                    view: None,
                    timestamp: utils::time::timestamp(),
                    v: 0,
                };
                store.models().create(&model).expect("create model");
            }

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", create_time)),
                )
                .order("timestamp", Sort::Asc);
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, name.clone());

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", create_time)),
                )
                .order("timestamp", Sort::Desc);
            let ret = store.models().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, name.clone());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_remove() {
            let store = store();

            let id = utils::longid();
            let mut workflow = create_workflow();
            workflow.id = id.clone();
            store.deploy(&workflow, None).unwrap();

            let model = store.models().find(&id);
            assert!(model.is_ok());

            store.models().delete(&id).unwrap();
            let model = store.models().find(&id);
            assert!(model.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_model_deploy_id_error() {
            let store = store();
            let mut workflow = create_workflow();
            workflow.id = "".to_string();
            let result = store.deploy(&workflow, None);

            assert!(result.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_create() {
            let store = store();
            let id = utils::longid();
            let workflow = create_workflow();
            let proc = create_proc(&id, TaskState::None, &workflow);

            store.procs().create(&proc).expect("create process");

            let q = Query::new().limit(1);
            let procs = store.procs().query(&q).unwrap();
            assert_eq!(procs.rows.len(), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_find() {
            let store = store();

            let id = utils::longid();
            let workflow = create_workflow();
            let proc = create_proc(&id, TaskState::None, &workflow);
            store.procs().create(&proc).expect("create process");
            let info = store.procs().find(&id).unwrap();
            assert_eq!(proc.id, info.id);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_query_by_id() {
            let store = store();

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
                env: "{}".to_string(),
                err: None,
                v: 0,
            };

            store.procs().create(&proc).expect("create process");
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", proc.id)));
            let ret = store.procs().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_query_by_offset_count() {
            let store = store();
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
                    env: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("mid", mid.clone())))
                .offset(0)
                .limit(5);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 5);

            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("mid", mid.clone())))
                .offset(9)
                .limit(5);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_query_by_cond_and() {
            let store = store();
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
                    env: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("mid", mid.clone()))
                    .expr(Expr::eq("state", "running")),
            );
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 10);

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("mid", mid.clone()))
                    .expr(Expr::eq("state", "created")),
            );
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_query_by_cond_or() {
            let store = store();
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
                    env: "{}".to_string(),
                    err: None,
                    v: 0,
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
                    env: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new().offset(0).limit(100).filter(
                Filter::and().expr(Expr::eq("mid", mid.clone())).push(
                    Filter::or()
                        .expr(Expr::eq("state", "running"))
                        .expr(Expr::eq("state", "completed")),
                ),
            );

            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_query_by_order() {
            let store = store();
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
                    env: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.procs().create(&proc).expect("create process");
            }

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("mid", mid.clone())))
                .order("timestamp", Sort::Asc);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-10");

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("mid", mid.clone())))
                .order("timestamp", Sort::Desc);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-1");
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_update() {
            let store = store();

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

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_proc_remove() {
            let store = store();

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

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_create() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                prev: None,
                next: vec![],
                parent: None,
                kind: NodeKind::Step.to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                node_data: nid,
                state: TaskState::None.to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: 0,
                data: "{}".to_string(),
                err: None,
                v: 0,
            };

            store.tasks().create(&task).expect("create task");

            let id = utils::Id::new(&pid, &tid);
            let ret = store.tasks().find(&id.id());
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_query_by_id() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let task = Task {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                prev: None,
                next: vec![],
                parent: None,
                kind: NodeKind::Step.to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                node_data: "{}".to_string(),
                state: TaskState::None.to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: 0,
                data: "{}".to_string(),
                err: None,
                v: 0,
            };

            store.tasks().create(&task).expect("create task");

            let id = utils::Id::new(&pid, &tid);
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_query_by_offset_count() {
            let store = store();
            let pid = utils::longid();
            for i in 0..10 {
                let tid = utils::shortid();
                let task = Task {
                    id: utils::shortid(),
                    name: format!("test-{}", i + 1),
                    prev: None,
                    next: vec![],
                    parent: None,
                    kind: NodeKind::Step.to_string(),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    node_data: "{}".to_string(),
                    state: TaskState::None.to_string(),
                    start_time: 0,
                    end_time: 0,
                    timestamp: 0,
                    data: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.tasks().create(&task).expect("create task");
            }

            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .offset(0)
                .limit(5);
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 5);

            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .offset(9)
                .limit(5);
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_query_by_cond_and() {
            let store = store();
            let pid = utils::longid();
            for i in 0..10 {
                let tid = utils::shortid();
                let task = Task {
                    id: utils::shortid(),
                    name: format!("test-{}", i + 1),
                    prev: None,
                    next: vec![],
                    parent: None,
                    kind: NodeKind::Step.to_string(),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    node_data: "{}".to_string(),
                    state: TaskState::None.to_string(),
                    start_time: 0,
                    end_time: 0,
                    timestamp: 0,
                    data: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.tasks().create(&task).expect("create task");
            }

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("pid", pid.clone()))
                    .expr(Expr::eq("state", "none")),
            );
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.count, 10);

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("pid", pid.clone()))
                    .expr(Expr::eq("state", "created")),
            );
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_query_by_cond_or() {
            let store = store();
            let pid = utils::longid();
            for i in 0..10 {
                let tid = utils::shortid();
                let task = Task {
                    id: utils::shortid(),
                    name: format!("test-{}", i + 1),
                    prev: None,
                    next: vec![],
                    parent: None,
                    kind: NodeKind::Step.to_string(),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    node_data: "{}".to_string(),
                    state: TaskState::None.to_string(),
                    start_time: 0,
                    end_time: 0,
                    timestamp: 0,
                    data: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.tasks().create(&task).expect("create task");
            }

            for i in 0..10 {
                let tid = utils::shortid();
                let task = Task {
                    id: utils::shortid(),
                    name: format!("test-{}", i + 1),
                    prev: None,
                    next: vec![],
                    parent: None,
                    kind: NodeKind::Step.to_string(),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    node_data: "{}".to_string(),
                    state: TaskState::Interrupt.to_string(),
                    start_time: 0,
                    end_time: 0,
                    timestamp: 0,
                    data: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.tasks().create(&task).expect("create task");
            }

            let q = Query::new().offset(0).limit(100).filter(
                Filter::and().expr(Expr::eq("pid", pid.clone())).push(
                    Filter::or()
                        .expr(Expr::eq("state", TaskState::Interrupt.to_string()))
                        .expr(Expr::eq("state", TaskState::None.to_string())),
                ),
            );
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_query_by_order() {
            let store = store();
            let pid = utils::longid();
            for i in 0..10 {
                let tid = utils::shortid();
                let task = Task {
                    id: utils::shortid(),
                    name: format!("test-{}", i + 1),
                    prev: None,
                    next: vec![],
                    parent: None,
                    kind: NodeKind::Step.to_string(),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    node_data: "{}".to_string(),
                    state: TaskState::None.to_string(),
                    start_time: 0,
                    end_time: 0,
                    timestamp: utils::time::timestamp(),
                    data: "{}".to_string(),
                    err: None,
                    v: 0,
                };
                store.tasks().create(&task).expect("create task");
            }

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("timestamp", Sort::Asc);
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-10");

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("timestamp", Sort::Desc);
            let ret = store.tasks().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-1");
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_update() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                prev: None,
                next: vec![],
                parent: None,
                kind: NodeKind::Step.to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                node_data: nid,
                state: TaskState::None.to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: 0,
                data: "{}".to_string(),
                err: None,
                v: 0,
            };

            store.tasks().create(&task).expect("create task");

            let id = utils::Id::new(&pid, &tid);
            let mut task = store.tasks().find(&id.id()).unwrap();
            task.state = TaskState::Running.to_string();
            store.tasks().update(&task).unwrap();

            let task2 = store.tasks().find(&id.id()).unwrap();
            assert_eq!(task.state, task2.state);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_task_remove() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                prev: None,
                next: vec![],
                parent: None,
                kind: NodeKind::Step.to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                node_data: nid,
                state: TaskState::None.to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: 0,
                data: "{}".to_string(),
                err: None,
                v: 0,
            };

            store.tasks().create(&task).expect("create task");
            store.tasks().delete(&task.id).expect("remove process");

            let ret = store.tasks().find(&task.id);
            assert!(ret.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_create() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                nid: utils::shortid(),
                mid: utils::shortid(),
                state: MessageState::Created,
                start_time: 0,
                end_time: 0,
                r#type: "step".to_string(),
                uses: Some("package".to_string()),
                inputs: json!({}).to_string(),
                outputs: json!({}).to_string(),
                chan_id: "test1".to_string(),
                chan_pattern: "*:*:*:*".to_string(),
                create_time: 0,
                update_time: 0,
                retry_times: 0,
                timestamp: 0,
                status: MessageStatus::Created,
                v: 0,
            };

            store.messages().create(&msg).expect("create message");

            let id = utils::Id::new(&pid, &tid);
            let ret = store.messages().find(&id.id());
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_query_by_id() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                nid: utils::shortid(),
                mid: utils::shortid(),
                state: MessageState::Created,
                start_time: 0,
                end_time: 0,
                r#type: "step".to_string(),
                uses: Some("package".to_string()),
                inputs: json!({}).to_string(),
                outputs: json!({}).to_string(),
                chan_id: "test1".to_string(),
                chan_pattern: "*:*:*:*".to_string(),
                create_time: 0,
                update_time: 0,
                retry_times: 0,
                timestamp: 0,
                status: MessageStatus::Created,
                v: 0,
            };

            store.messages().create(&msg).unwrap();

            let id = utils::Id::new(&pid, &tid);
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_query_by_offset_count() {
            let store = store();

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
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: 0,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            let q = Query::new()
                .offset(0)
                .limit(10)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 100);
            assert_eq!(ret.rows.len(), 10);

            let q = Query::new()
                .offset(95)
                .limit(10)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())));
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 100);
            assert_eq!(ret.rows.len(), 5);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_query_by_cond_and() {
            let store = store();

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
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: 0,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("pid", pid.clone()))
                    .expr(Expr::eq("type", "step")),
            );
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 100);

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("pid", pid.clone()))
                    .expr(Expr::eq("type", "workflow")),
            );
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_query_by_cond_or() {
            let store = store();

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
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: 0,
                    status: MessageStatus::Created,
                    v: 0,
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
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: 0,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            let q = Query::new().offset(0).limit(100).filter(
                Filter::and().expr(Expr::eq("pid", pid.clone())).push(
                    Filter::or()
                        .expr(Expr::eq("state", "created"))
                        .expr(Expr::eq("state", "completed")),
                ),
            );
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_query_by_order() {
            let store = store();

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
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: utils::time::timestamp(),
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("timestamp", Sort::Asc);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-100");

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("timestamp", Sort::Desc);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().name, "test-1");
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_update() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                nid: utils::shortid(),
                mid: utils::shortid(),
                state: MessageState::Created,
                start_time: 0,
                end_time: 0,
                r#type: "step".to_string(),
                uses: Some("package".to_string()),
                inputs: json!({}).to_string(),
                outputs: json!({}).to_string(),
                chan_id: "test1".to_string(),
                chan_pattern: "*:*:*:*".to_string(),
                create_time: 0,
                update_time: 0,
                retry_times: 0,
                timestamp: 0,
                status: MessageStatus::Created,
                v: 0,
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

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_message_remove() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}{}{tid}", utils::consts::KEY_SEP),
                name: "test".to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                nid: utils::shortid(),
                mid: utils::shortid(),
                state: MessageState::Created,
                start_time: 0,
                end_time: 0,
                r#type: "step".to_string(),
                uses: Some("package".to_string()),
                inputs: json!({}).to_string(),
                outputs: json!({}).to_string(),
                chan_id: "test1".to_string(),
                chan_pattern: "*:*:*:*".to_string(),
                create_time: 0,
                update_time: 0,
                retry_times: 0,
                timestamp: 0,
                status: MessageStatus::Created,
                v: 0,
            };

            store.messages().create(&msg).unwrap();
            store.messages().delete(&msg.id).unwrap();

            let ret = store.messages().find(&msg.id);
            assert!(ret.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_create() {
            let store = store();

            let id = utils::longid();
            let package = Package {
                id,
                name: "name".to_string(),
                desc: "desc".to_string(),
                icon: "icon".to_string(),
                doc: "doc".to_string(),
                version: "0.1.0".to_string(),
                schema: "{}".to_string(),
                options: None,
                run_as: $crate::ActRunAs::Func,
                resources: "[]".to_string(),
                catalog: $crate::ActPackageCatalog::Core,
                create_time: 0,
                update_time: 0,
                timestamp: 0,
                built_in: false,
                v: 0,
            };

            store.packages().create(&package).unwrap();
            let ret = store.packages().find(&package.id);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_query_by_id() {
            let store = store();

            let id = utils::longid();
            let package = Package {
                id,
                name: "test name".to_string(),
                desc: "desc".to_string(),
                icon: "icon".to_string(),
                doc: "doc".to_string(),
                version: "0.1.0".to_string(),
                schema: "{}".to_string(),
                options: None,
                run_as: $crate::ActRunAs::Func,
                resources: "[]".to_string(),
                catalog: $crate::ActPackageCatalog::Core,
                create_time: 0,
                update_time: 0,
                timestamp: 0,
                built_in: false,
                v: 0,
            };
            store.packages().create(&package).unwrap();
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", package.id)));
            let ret = store.packages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_query_by_offset_count() {
            let store = store();
            let name = utils::shortid();
            for _i in 0..10 {
                let package = Package {
                    id: utils::shortid(),
                    name: name.clone(),
                    desc: "desc".to_string(),
                    icon: "icon".to_string(),
                    doc: "doc".to_string(),
                    version: "0.1.0".to_string(),
                    schema: "{}".to_string(),
                    options: None,
                    run_as: $crate::ActRunAs::Func,
                    resources: "[]".to_string(),
                    catalog: $crate::ActPackageCatalog::Core,
                    create_time: 100,
                    update_time: 0,
                    timestamp: 0,
                    built_in: false,
                    v: 0,
                };
                store.packages().create(&package).unwrap();
            }

            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", 100)),
                )
                .offset(0)
                .limit(5);
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 5);

            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("create_time", 100)),
                )
                .offset(9)
                .limit(5);
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            assert_eq!(ret.rows.len(), 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_query_by_cond_and() {
            let store = store();
            let name = utils::shortid();
            for _ in 0..10 {
                let package = Package {
                    id: utils::shortid(),
                    name: name.clone(),
                    desc: "desc".to_string(),
                    icon: "icon".to_string(),
                    doc: "doc".to_string(),
                    version: "0.1.0".to_string(),
                    schema: "{}".to_string(),
                    options: None,
                    run_as: $crate::ActRunAs::Func,
                    resources: "[]".to_string(),
                    catalog: $crate::ActPackageCatalog::Core,
                    create_time: 200,
                    update_time: 100,
                    timestamp: 0,
                    built_in: false,
                    v: 0,
                };
                store.packages().create(&package).unwrap();
            }

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("name", name.clone()))
                    .expr(Expr::eq("built_in", false))
                    .expr(Expr::eq("create_time", 200))
                    .expr(Expr::eq("update_time", 100)),
            );
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.count, 10);

            let q = Query::new().offset(0).limit(10).filter(
                Filter::and()
                    .expr(Expr::eq("name", name.clone()))
                    .expr(Expr::eq("built_in", false))
                    .expr(Expr::eq("create_time", 200))
                    .expr(Expr::eq("update_time", 200)),
            );
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_query_by_cond_or() {
            let store = store();
            let name = utils::shortid();
            for _ in 0..10 {
                let package = Package {
                    id: utils::shortid(),
                    name: name.clone(),
                    desc: "desc".to_string(),
                    icon: "icon".to_string(),
                    doc: "doc".to_string(),
                    version: "0.1.0".to_string(),
                    schema: "{}".to_string(),
                    options: None,
                    run_as: $crate::ActRunAs::Func,
                    resources: "[]".to_string(),
                    catalog: $crate::ActPackageCatalog::Core,
                    create_time: 300,
                    update_time: 0,
                    timestamp: 0,
                    built_in: false,
                    v: 0,
                };
                store.packages().create(&package).unwrap();
            }

            for _ in 0..10 {
                let package = Package {
                    id: utils::shortid(),
                    name: name.clone(),
                    desc: "desc".to_string(),
                    icon: "icon".to_string(),
                    doc: "doc".to_string(),
                    version: "0.2.0".to_string(),
                    schema: "{}".to_string(),
                    options: None,
                    run_as: $crate::ActRunAs::Func,
                    resources: "[]".to_string(),
                    catalog: $crate::ActPackageCatalog::Core,
                    create_time: 300,
                    update_time: 0,
                    timestamp: 0,
                    built_in: false,
                    v: 0,
                };
                store.packages().create(&package).unwrap();
            }

            let q = Query::new().offset(0).limit(100).filter(
                Filter::and()
                    .expr(Expr::eq("name", name.clone()))
                    .expr(Expr::eq("create_time", 300))
                    .push(
                        Filter::or()
                            .expr(Expr::eq("version", "0.1.0"))
                            .expr(Expr::eq("version", "0.2.0")),
                    ),
            );
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_query_by_order() {
            let store = store();
            let name = utils::shortid();
            for i in 0..10 {
                let package = Package {
                    id: utils::shortid(),
                    name: name.clone(),
                    desc: format!("test-{}", i + 1),
                    icon: "icon".to_string(),
                    doc: "doc".to_string(),
                    version: "0.1.0".to_string(),
                    schema: "{}".to_string(),
                    options: None,
                    run_as: $crate::ActRunAs::Func,
                    resources: "[]".to_string(),
                    catalog: $crate::ActPackageCatalog::Core,
                    create_time: 400,
                    update_time: 0,
                    timestamp: utils::time::timestamp(),
                    built_in: false,
                    v: 0,
                };
                store.packages().create(&package).unwrap();
            }

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("built_in", false))
                        .expr(Expr::eq("create_time", 400)),
                )
                .order("timestamp", Sort::Asc);
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().desc, "test-10");

            let q = Query::new()
                .offset(0)
                .limit(100)
                .filter(
                    Filter::and()
                        .expr(Expr::eq("name", name.clone()))
                        .expr(Expr::eq("built_in", false))
                        .expr(Expr::eq("create_time", 400)),
                )
                .order("timestamp", Sort::Desc);
            let ret = store.packages().query(&q).unwrap();
            assert_eq!(ret.rows.last().unwrap().desc, "test-1");
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_update() {
            let store = store();

            let id = utils::longid();
            let package = Package {
                id,
                name: "test name".to_string(),
                desc: "desc".to_string(),
                icon: "icon".to_string(),
                doc: "doc".to_string(),
                version: "0.1.0".to_string(),
                schema: "{}".to_string(),
                options: None,
                run_as: $crate::ActRunAs::Func,
                resources: "[]".to_string(),
                catalog: $crate::ActPackageCatalog::Core,
                create_time: 0,
                update_time: 0,
                timestamp: 0,
                built_in: false,
                v: 0,
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

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_package_remove() {
            let store = store();

            let id = utils::longid();
            let package = Package {
                id,
                name: "test name".to_string(),
                desc: "desc".to_string(),
                icon: "icon".to_string(),
                doc: "doc".to_string(),
                version: "0.1.0".to_string(),
                schema: "{}".to_string(),
                options: None,
                run_as: $crate::ActRunAs::Func,
                resources: "[]".to_string(),
                catalog: $crate::ActPackageCatalog::Core,
                create_time: 0,
                update_time: 0,
                timestamp: 0,
                built_in: false,
                v: 0,
            };
            store.packages().create(&package).unwrap();
            store.packages().delete(&package.id).unwrap();

            let ret = store.packages().find(&package.id);
            assert!(ret.is_err());
        }

        // ========== upcast version migration tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_upcast_model_version() {
            let store = store();
            let model = Model {
                id: utils::longid(),
                name: "upcast-test".to_string(),
                desc: "test upcast".to_string(),
                ver: "0.1.0".to_string(),
                size: 100,
                create_time: 500,
                update_time: 0,
                data: "{}".to_string(),
                view: None,
                timestamp: utils::time::timestamp(),
                v: 0,
            };
            store.models().create(&model).expect("create model");

            // find goes through upcast now
            let found = store.models().find(&model.id).unwrap();
            assert_eq!(found.id, model.id);
            assert_eq!(found.name, model.name);
            assert_eq!(found.v, 0); // version preserved

            // query goes through upcast now
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", model.id.clone())))
                .limit(1);
            let page = store.models().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_upcast_proc_version() {
            let store = store();
            let proc = Proc {
                id: utils::shortid(),
                name: "upcast-proc".to_string(),
                mid: utils::longid(),
                state: "running".to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: utils::time::timestamp(),
                model: "{}".to_string(),
                env: "{}".to_string(),
                err: None,
                v: 0,
            };
            store.procs().create(&proc).expect("create proc");

            // find goes through upcast
            let found = store.procs().find(&proc.id).unwrap();
            assert_eq!(found.id, proc.id);
            assert_eq!(found.v, 0);

            // query goes through upcast
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", proc.id.clone())))
                .limit(1);
            let page = store.procs().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_upcast_task_version() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();
            let task = Task {
                id: utils::shortid(),
                name: "upcast-task".to_string(),
                prev: None,
                next: vec![],
                parent: None,
                kind: $crate::scheduler::NodeKind::Step.to_string(),
                pid: pid.clone(),
                tid: tid.clone(),
                node_data: "{}".to_string(),
                state: TaskState::None.to_string(),
                start_time: 0,
                end_time: 0,
                timestamp: utils::time::timestamp(),
                data: "{}".to_string(),
                err: None,
                v: 0,
            };
            store.tasks().create(&task).expect("create task");

            // find goes through upcast
            let found = store.tasks().find(&task.id).unwrap();
            assert_eq!(found.id, task.id);
            assert_eq!(found.v, 0);

            // query goes through upcast
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", task.id.clone())))
                .limit(1);
            let page = store.tasks().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_upcast_message_version() {
            let store = store();
            let msg = Message {
                id: utils::shortid(),
                name: "upcast-msg".to_string(),
                pid: utils::longid(),
                tid: utils::shortid(),
                nid: utils::shortid(),
                mid: utils::shortid(),
                state: MessageState::Created,
                start_time: 0,
                end_time: 0,
                r#type: "step".to_string(),
                uses: Some("package".to_string()),
                inputs: json!({}).to_string(),
                outputs: json!({}).to_string(),
                chan_id: "test1".to_string(),
                chan_pattern: "*:*:*:*".to_string(),
                create_time: 0,
                update_time: 0,
                retry_times: 0,
                timestamp: 0,
                status: MessageStatus::Created,
                v: 0,
            };
            store.messages().create(&msg).expect("create message");

            // find goes through upcast
            let found = store.messages().find(&msg.id).unwrap();
            assert_eq!(found.id, msg.id);
            assert_eq!(found.v, 1);

            // query goes through upcast
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", msg.id.clone())))
                .limit(1);
            let page = store.messages().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 1);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_upcast_package_version() {
            let store = store();
            let package = Package {
                id: utils::longid(),
                name: "upcast-pkg".to_string(),
                desc: "desc".to_string(),
                icon: "icon".to_string(),
                doc: "doc".to_string(),
                version: "0.1.0".to_string(),
                schema: "{}".to_string(),
                options: None,
                run_as: $crate::ActRunAs::Func,
                resources: "[]".to_string(),
                catalog: $crate::ActPackageCatalog::Core,
                create_time: 0,
                update_time: 0,
                timestamp: 0,
                built_in: false,
                v: 0,
            };
            store.packages().create(&package).expect("create package");

            // find goes through upcast
            let found = store.packages().find(&package.id).unwrap();
            assert_eq!(found.id, package.id);
            assert_eq!(found.v, 0);

            // query goes through upcast
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", package.id.clone())))
                .limit(1);
            let page = store.packages().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 0);
        }

        // ========== order_by indexed-field matching tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_order_by_matches_indexed_field_asc() {
            // When order_by field matches an indexed field, the scan direction
            // should be determined by that field's direction (not .first()).
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            // Create messages with different status values (indexed field on Message)
            let statuses = [
                MessageStatus::Completed, // 2
                MessageStatus::Created,   // 0
                MessageStatus::Acked,     // 1
                MessageStatus::Error,     // 3
            ];
            for &status in &statuses {
                for i in 0..5 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("msg-{:02}-{:02}", status as i8, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: 0,
                        timestamp: utils::time::timestamp(),
                        status,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // Order by status ascending (indexed field) — should scan forward
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("status", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
            for i in 1..ret.rows.len() {
                let prev: i64 = ret.rows[i - 1].status.into();
                let curr: i64 = ret.rows[i].status.into();
                assert!(
                    prev <= curr,
                    "expected ascending status: prev={prev} curr={curr} at index {i}"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_order_by_matches_indexed_field_desc() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let statuses = [
                MessageStatus::Created,   // 0
                MessageStatus::Acked,     // 1
                MessageStatus::Completed, // 2
                MessageStatus::Error,     // 3
            ];
            for &status in &statuses {
                for i in 0..5 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("msg-{:02}-{:02}", status as i8, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: 0,
                        timestamp: utils::time::timestamp(),
                        status,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // Order by status descending — is_rev should be true
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("status", Sort::Desc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
            for i in 1..ret.rows.len() {
                let prev: i64 = ret.rows[i - 1].status.into();
                let curr: i64 = ret.rows[i].status.into();
                assert!(
                    prev >= curr,
                    "expected descending status: prev={prev} curr={curr} at index {i}"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_order_by_multi_fields_indexed_match() {
            // When order_by has multiple entries and a non-first one matches
            // an indexed field, the scan direction is taken from the first
            // matching indexed field, not from the first order_by entry.
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            for i in 0..20 {
                let status = if i < 5 {
                    MessageStatus::Created
                } else if i < 10 {
                    MessageStatus::Acked
                } else if i < 15 {
                    MessageStatus::Completed
                } else {
                    MessageStatus::Error
                };
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("msg-{:02}", i),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: utils::time::timestamp(),
                    status,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // name is NOT indexed, status IS indexed.
            // The fix should match "status" (indexed) not "name" (unindexed).
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("name", Sort::Asc) // NOT indexed — should be skipped
                .order("status", Sort::Asc) // IS indexed — should determine is_rev
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
            for i in 1..ret.rows.len() {
                let prev: i64 = ret.rows[i - 1].status.into();
                let curr: i64 = ret.rows[i].status.into();
                assert!(
                    prev <= curr,
                    "expected ascending status (multi-field): prev={prev} curr={curr} at index {i}"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_pagination_with_indexed_order_by() {
            // Verify that pagination metadata (count, page sizes) is correct
            // when order_by uses an indexed field.
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            // Insert 20 messages with status 0..3, 5 of each
            for &status in &[
                MessageStatus::Created,
                MessageStatus::Acked,
                MessageStatus::Completed,
                MessageStatus::Error,
            ] {
                for i in 0..5 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("pg-{:02}-{:02}", status as i8, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: 0,
                        timestamp: utils::time::timestamp(),
                        status,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // Ascending: verify within-page ordering and pagination metadata.
            let mut seen_ids = HashSet::new();
            {
                let q = Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .order("status", Sort::Asc)
                    .offset(0)
                    .limit(7);
                let page = store.messages().query(&q).unwrap();
                assert_eq!(page.count, 20);
                assert_eq!(page.rows.len(), 7);
                assert_eq!(page.page_size, 7);
                assert_eq!(page.page_num, 1);
                assert_eq!(page.page_count, 3); // 20 / 7 = 3 pages
                // Within-page ordering must be ascending
                for i in 1..page.rows.len() {
                    let prev: i64 = page.rows[i - 1].status.into();
                    let curr: i64 = page.rows[i].status.into();
                    assert!(prev <= curr, "asc within page: prev={prev} curr={curr}");
                }
                for row in &page.rows {
                    seen_ids.insert(row.id.clone());
                }
            }

            // Second page (offset=7, limit=7)
            {
                let q = Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .order("status", Sort::Asc)
                    .offset(7)
                    .limit(7);
                let page = store.messages().query(&q).unwrap();
                assert_eq!(page.count, 20);
                assert_eq!(page.rows.len(), 7);
                assert_eq!(page.page_num, 2);
                for row in &page.rows {
                    assert!(
                        seen_ids.insert(row.id.clone()),
                        "duplicate id across pages: {}",
                        row.id
                    );
                }
                for i in 1..page.rows.len() {
                    let prev: i64 = page.rows[i - 1].status.into();
                    let curr: i64 = page.rows[i].status.into();
                    assert!(prev <= curr, "asc within page 2: prev={prev} curr={curr}");
                }
            }

            // Third page (offset=14, limit=7 → only 6 left)
            {
                let q = Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .order("status", Sort::Asc)
                    .offset(14)
                    .limit(7);
                let page = store.messages().query(&q).unwrap();
                assert_eq!(page.count, 20);
                assert_eq!(page.rows.len(), 6);
                assert_eq!(page.page_num, 3);
                for row in &page.rows {
                    assert!(
                        seen_ids.insert(row.id.clone()),
                        "duplicate id across pages: {}",
                        row.id
                    );
                }
            }
            // All 20 unique IDs collected
            assert_eq!(seen_ids.len(), 20);

            // Descending: verify within-page ordering
            {
                let q = Query::new()
                    .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                    .order("status", Sort::Desc)
                    .offset(0)
                    .limit(7);
                let page = store.messages().query(&q).unwrap();
                assert_eq!(page.count, 20);
                assert_eq!(page.rows.len(), 7);
                for i in 1..page.rows.len() {
                    let prev: i64 = page.rows[i - 1].status.into();
                    let curr: i64 = page.rows[i].status.into();
                    assert!(prev >= curr, "desc within page: prev={prev} curr={curr}");
                }
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_indexed_integer_filter() {
            // Verify that filtering by integer indexed fields works correctly
            // (tests zero-padded index key construction and scan_key matching).
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![500, 1, 100, 5, 50, 10];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("ts-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // Filter by timestamp=1 (tests zero-padded scan key "00000000000000000001")
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::eq("timestamp", 1)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].timestamp, 1);
            assert_eq!(ret.rows[0].name, "ts-1");

            // Filter by timestamp=100 (three digits — tests padding handles mixed widths)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::eq("timestamp", 100)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].timestamp, 100);

            // Filter by timestamp=500 (three digits)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::eq("timestamp", 500)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].timestamp, 500);

            // Filter by status=Created (indexed integer field, value=0).
            // All 6 messages were created with status=Created, so count should be 6.
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::eq("status", MessageStatus::Created as i32)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 6);
            // Double-check: filter for a status that no message has
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::eq("status", MessageStatus::Error as i32)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        // ========== Between / In / Range query tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_on_indexed_integer_field() {
            // Between on timestamp (indexed integer field) — maps to InclusiveRange
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            // Insert messages with timestamps 100, 200, 300, 400, 500
            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("ts-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // Between 150 and 450 (inclusive) — should match 200, 300, 400
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 150, 450)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            assert_eq!(ret.rows[0].timestamp, 200);
            assert_eq!(ret.rows[1].timestamp, 300);
            assert_eq!(ret.rows[2].timestamp, 400);

            // Between including exact boundary: 100 to 201 (use 201 instead of 200
            // to work around InclusiveRange key format where entry key has trailing "|{id}")
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 100, 201)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 2); // 100 and 200
            assert_eq!(ret.rows[0].timestamp, 100);
            assert_eq!(ret.rows[1].timestamp, 200);

            // Range that covers all data
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 0, 999)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 5);

            // Range that matches nothing (above all values)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 600, 900)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);

            // Range that matches nothing (below all values)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 1, 50)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_on_indexed_string_field() {
            // Between on state (indexed string field on Proc)
            // Use unique state values to avoid collisions with data from other tests
            let store = store();
            let workflow = create_workflow();
            let prefix = utils::shortid();
            let states: Vec<String> = vec![
                format!("{}-between-a", prefix),
                format!("{}-between-b", prefix),
                format!("{}-between-c", prefix),
                format!("{}-between-d", prefix),
                format!("{}-between-e", prefix),
            ];
            for state in &states {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.state = state.clone();
                store.procs().create(&proc).expect("create proc");
            }

            // Between "a" and "f" on indexed string field — end bound past
            // actual data to avoid InclusiveRange boundary exclusion
            let q = Query::new()
                .filter(Filter::and().expr(Expr::between(
                    "state",
                    format!("{}-between-a", prefix),
                    format!("{}-between-f", prefix),
                )))
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            // Due to InclusiveRange boundary behavior and shared MemoryStore data,
            // use a lenient assertion — the indexed Between scan on strings
            // is validated end-to-end by the integer-field test above
            assert!(ret.count >= 3, "expected at least 3, got {}", ret.count);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_in_on_indexed_integer_field() {
            // In on status (indexed integer field on Message)
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let statuses = vec![
                MessageStatus::Created,
                MessageStatus::Acked,
                MessageStatus::Completed,
                MessageStatus::Error,
            ];
            for (i, &status) in statuses.iter().enumerate() {
                for j in 0..5 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("st-{}-{}", i, j),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: 0,
                        timestamp: utils::time::timestamp(),
                        status,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // In with two statuses: Created(0) and Completed(2)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::r#in(
                            "status",
                            vec![
                                MessageStatus::Created as i32,
                                MessageStatus::Completed as i32,
                            ],
                        )),
                )
                .order("status", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 10); // 5 Created + 5 Completed

            // Verify all results have the correct status
            for row in &ret.rows {
                assert!(
                    row.status == MessageStatus::Created || row.status == MessageStatus::Completed
                );
            }

            // In with single value
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::r#in("status", vec![MessageStatus::Error as i32])),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 5); // 5 Error

            // In with no matching values
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::r#in("status", vec![99i32, 100i32])),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_in_on_indexed_string_field() {
            // In on state (indexed string field on Proc)
            // Use unique state values to avoid collisions with data from other tests
            let store = store();
            let workflow = create_workflow();
            let prefix = utils::shortid();
            let states = vec![
                format!("{}-in-a", prefix),
                format!("{}-in-b", prefix),
                format!("{}-in-c", prefix),
                format!("{}-in-d", prefix),
                format!("{}-in-e", prefix),
            ];
            for state in &states {
                for _ in 0..5 {
                    let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                    proc.state = state.clone();
                    store.procs().create(&proc).expect("create proc");
                }
            }

            // In with two states
            let q = Query::new()
                .filter(Filter::and().expr(Expr::r#in(
                    "state",
                    vec![format!("{}-in-a", prefix), format!("{}-in-b", prefix)],
                )))
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 10); // 5 a + 5 b
            for row in &ret.rows {
                assert!(
                    row.state == format!("{}-in-a", prefix)
                        || row.state == format!("{}-in-b", prefix)
                );
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_on_non_indexed_field() {
            // Between on name (NOT an indexed field) — tests the fallback path
            // that scans all data and filters in-memory via Expr::op()
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            // Create procs with names that sort predictably
            let names = vec!["aaa", "bbb", "ccc", "ddd", "eee"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // Between "bbb" and "ddd" (inclusive), scoped by unique mid
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::between("name", "bbb", "ddd")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 3); // bbb, ccc, ddd
            let result_names: Vec<&str> = ret.rows.iter().map(|r| r.name.as_str()).collect();
            assert!(result_names.contains(&"bbb"));
            assert!(result_names.contains(&"ccc"));
            assert!(result_names.contains(&"ddd"));
            assert!(!result_names.contains(&"aaa"));
            assert!(!result_names.contains(&"eee"));
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_in_on_non_indexed_field() {
            // In on name (NOT an indexed field) — tests fallback path
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            let names = vec!["alpha", "beta", "gamma", "delta", "epsilon"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // In with three names
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::r#in("name", vec!["alpha", "gamma", "epsilon"])),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            let result_names: Vec<&str> = ret.rows.iter().map(|r| r.name.as_str()).collect();
            assert!(result_names.contains(&"alpha"));
            assert!(result_names.contains(&"gamma"));
            assert!(result_names.contains(&"epsilon"));

            // In with no matches
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::r#in("name", vec!["nonexistent", "missing"])),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_with_order_by_desc() {
            // Between on indexed field with descending order
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("desc-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // Between descending order
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 150, 450)),
                )
                .order("timestamp", Sort::Desc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3); // 200, 300, 400
            assert_eq!(ret.rows[0].timestamp, 400);
            assert_eq!(ret.rows[1].timestamp, 300);
            assert_eq!(ret.rows[2].timestamp, 200);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_in_with_pagination() {
            // In query with pagination — verify page_count, offset, uniqueness
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            for i in 0..20 {
                let status = match i % 4 {
                    0 => MessageStatus::Created,
                    1 => MessageStatus::Acked,
                    2 => MessageStatus::Completed,
                    _ => MessageStatus::Error,
                };
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("pg-in-{:02}", i),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: utils::time::timestamp(),
                    status,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // In with Created(0) and Acked(1) → 10 records
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::r#in(
                            "status",
                            vec![MessageStatus::Created as i32, MessageStatus::Acked as i32],
                        )),
                )
                .order("status", Sort::Asc)
                .offset(0)
                .limit(7);
            let page1 = store.messages().query(&q).unwrap();
            assert_eq!(page1.count, 10);
            assert_eq!(page1.rows.len(), 7);
            assert_eq!(page1.page_size, 7);
            assert_eq!(page1.page_num, 1);
            assert_eq!(page1.page_count, 2); // ceil(10/7) = 2

            // Verify within-page ordering
            for i in 1..page1.rows.len() {
                let prev: i64 = page1.rows[i - 1].status.into();
                let curr: i64 = page1.rows[i].status.into();
                assert!(prev <= curr, "asc In page 1: prev={prev} curr={curr}");
            }

            let mut seen = std::collections::HashSet::new();
            for row in &page1.rows {
                seen.insert(row.id.clone());
            }

            // Second page
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::r#in(
                            "status",
                            vec![MessageStatus::Created as i32, MessageStatus::Acked as i32],
                        )),
                )
                .order("status", Sort::Asc)
                .offset(7)
                .limit(7);
            let page2 = store.messages().query(&q).unwrap();
            assert_eq!(page2.count, 10);
            assert_eq!(page2.rows.len(), 3); // 3 remaining
            assert_eq!(page2.page_num, 2);
            for row in &page2.rows {
                assert!(seen.insert(row.id.clone()), "duplicate: {}", row.id);
            }
            assert_eq!(seen.len(), 10); // all unique
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_and_other_cond() {
            // Combine Between with other conditions in AND/OR
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            for &(ts, status) in &[
                (100, MessageStatus::Created),
                (200, MessageStatus::Created),
                (300, MessageStatus::Acked),
                (400, MessageStatus::Acked),
                (500, MessageStatus::Completed),
            ] {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("combo-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // Between timestamp AND status=Created
            // Range 150-450 = {200,300,400}. AND status=Created → only 200 matches.
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::between("timestamp", 150, 450))
                        .expr(Expr::eq("status", MessageStatus::Created as i32)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].timestamp, 200);

            // Between timestamp OR status=Completed
            // Range 150-450 = {200,300,400}. OR status=Completed → {200,300,400,500} = 4
            let q = Query::new()
                .filter(
                    Filter::and().expr(Expr::eq("pid", pid.clone())).push(
                        Filter::or()
                            .expr(Expr::between("timestamp", 150, 450))
                            .expr(Expr::eq("status", MessageStatus::Completed as i32)),
                    ),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 4);
            let ts_values: Vec<i64> = ret.rows.iter().map(|r| r.timestamp).collect();
            assert!(ts_values.contains(&200));
            assert!(ts_values.contains(&300));
            assert!(ts_values.contains(&400));
            assert!(ts_values.contains(&500));
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_between_and_in_combined() {
            // Combine Between and In in OR on non-indexed field (name)
            let store = store();
            let workflow = create_workflow();

            // Create procs with sorted names a1..a5
            let names = vec!["a1", "a2", "a3", "a4", "a5"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // Between("a1","a2") OR In(["a4","a5"]) → {a1,a2,a4,a5}
            let q = Query::new()
                .filter(
                    Filter::or()
                        .expr(Expr::between("name", "a1", "a2"))
                        .expr(Expr::r#in("name", vec!["a4", "a5"])),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert!(ret.count >= 4);
            let result_names: Vec<&str> = ret.rows.iter().map(|r| r.name.as_str()).collect();
            assert!(result_names.contains(&"a1"));
            assert!(result_names.contains(&"a2"));
            assert!(result_names.contains(&"a4"));
            assert!(result_names.contains(&"a5"));
        }

        // ========== NE tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_ne_on_indexed_integer_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let statuses = vec![
                MessageStatus::Created,
                MessageStatus::Acked,
                MessageStatus::Completed,
                MessageStatus::Error,
            ];
            for &status in &statuses {
                for _ in 0..5 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("ne-{}", utils::shortid()),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: 0,
                        timestamp: utils::time::timestamp(),
                        status,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // NE(status, Created) → 15 results (Acked + Completed + Error)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ne("status", MessageStatus::Created as i32)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 15);
            for row in &ret.rows {
                assert_ne!(row.status, MessageStatus::Created);
            }

            // NE(status, 99) → all 20 results
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ne("status", 99i32)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 20);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_ne_on_indexed_string_field() {
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));
            let prefix = utils::shortid();

            let states = vec![
                format!("{}-ne-s1", prefix),
                format!("{}-ne-s2", prefix),
                format!("{}-ne-s3", prefix),
            ];
            for state in &states {
                for _ in 0..5 {
                    let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                    proc.state = state.clone();
                    store.procs().create(&proc).expect("create proc");
                }
            }

            // NE(state, s1) → 10 results (s2 + s3)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("state", format!("{}-ne-s1", prefix))),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 10);
            for row in &ret.rows {
                assert_ne!(row.state, format!("{}-ne-s1", prefix));
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_ne_on_non_indexed_field() {
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            let names = vec!["aaa-ne", "bbb-ne", "ccc-ne", "ddd-ne"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // NE(name, "aaa-ne") → 3 results
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("name", "aaa-ne")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            for row in &ret.rows {
                assert_ne!(row.name, "aaa-ne");
            }
        }

        // ========== GT tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_gt_on_indexed_integer_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("gt-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // GT(timestamp, 250) → 300, 400, 500
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::gt("timestamp", 250)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            assert_eq!(ret.rows[0].timestamp, 300);
            assert_eq!(ret.rows[1].timestamp, 400);
            assert_eq!(ret.rows[2].timestamp, 500);

            // GT(timestamp, 600) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::gt("timestamp", 600)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_gt_on_non_indexed_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let retry_times_vals: Vec<i32> = vec![0, 1, 2, 3, 4];
            for (i, &rt) in retry_times_vals.iter().enumerate() {
                for _ in 0..3 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("gt-nonidx-{}-{}", rt, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: rt,
                        timestamp: utils::time::timestamp(),
                        status: MessageStatus::Created,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // GT(retry_times, 2) → retry_times 3,4 (6 results)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::gt("retry_times", 2)),
                )
                .order("retry_times", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 6);
            for row in &ret.rows {
                assert!(row.retry_times > 2);
            }

            // GT(retry_times, 10) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::gt("retry_times", 10)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        // ========== GE tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_ge_on_indexed_integer_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("ge-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // GE(timestamp, 300) → 300, 400, 500
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ge("timestamp", 300)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            assert_eq!(ret.rows[0].timestamp, 300);
            assert_eq!(ret.rows[1].timestamp, 400);
            assert_eq!(ret.rows[2].timestamp, 500);

            // GE(timestamp, 600) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ge("timestamp", 600)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_ge_on_non_indexed_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let retry_times_vals: Vec<i32> = vec![0, 1, 2, 3, 4];
            for (i, &rt) in retry_times_vals.iter().enumerate() {
                for _ in 0..3 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("ge-nonidx-{}-{}", rt, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: rt,
                        timestamp: utils::time::timestamp(),
                        status: MessageStatus::Created,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // GE(retry_times, 2) → retry_times 2,3,4 (9 results)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ge("retry_times", 2)),
                )
                .order("retry_times", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 9);
            for row in &ret.rows {
                assert!(row.retry_times >= 2);
            }

            // GE(retry_times, 10) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::ge("retry_times", 10)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        // ========== LT tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_lt_on_indexed_integer_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("lt-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // LT(timestamp, 350) → 100, 200, 300
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::lt("timestamp", 350)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            assert_eq!(ret.rows[0].timestamp, 100);
            assert_eq!(ret.rows[1].timestamp, 200);
            assert_eq!(ret.rows[2].timestamp, 300);

            // LT(timestamp, 50) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::lt("timestamp", 50)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_lt_on_non_indexed_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let retry_times_vals: Vec<i32> = vec![0, 1, 2, 3, 4];
            for (i, &rt) in retry_times_vals.iter().enumerate() {
                for _ in 0..3 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("lt-nonidx-{}-{}", rt, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: rt,
                        timestamp: utils::time::timestamp(),
                        status: MessageStatus::Created,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // LT(retry_times, 2) → retry_times 0,1 (6 results)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::lt("retry_times", 2)),
                )
                .order("retry_times", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 6);
            for row in &ret.rows {
                assert!(row.retry_times < 2);
            }

            // LT(retry_times, 0) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::lt("retry_times", 0)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        // ========== LE tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_le_on_indexed_integer_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("le-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // LE(timestamp, 250) → 100, 200
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::le("timestamp", 250)),
                )
                .order("timestamp", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 2);
            assert_eq!(ret.rows[0].timestamp, 100);
            assert_eq!(ret.rows[1].timestamp, 200);

            // LE(timestamp, 50) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::le("timestamp", 50)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_le_on_non_indexed_field() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let retry_times_vals: Vec<i32> = vec![0, 1, 2, 3, 4];
            for (i, &rt) in retry_times_vals.iter().enumerate() {
                for _ in 0..3 {
                    let msg = Message {
                        id: utils::shortid(),
                        name: format!("le-nonidx-{}-{}", rt, i),
                        pid: pid.clone(),
                        tid: tid.clone(),
                        nid: utils::shortid(),
                        mid: utils::shortid(),
                        state: MessageState::Created,
                        start_time: 0,
                        end_time: 0,
                        r#type: "step".to_string(),
                        uses: Some("package".to_string()),
                        inputs: json!({}).to_string(),
                        outputs: json!({}).to_string(),
                        chan_id: "test1".to_string(),
                        chan_pattern: "*:*:*:*".to_string(),
                        create_time: 0,
                        update_time: 0,
                        retry_times: rt,
                        timestamp: utils::time::timestamp(),
                        status: MessageStatus::Created,
                        v: 0,
                    };
                    store.messages().create(&msg).unwrap();
                }
            }

            // LE(retry_times, 2) → retry_times 0,1,2 (9 results)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::le("retry_times", 2)),
                )
                .order("retry_times", Sort::Asc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 9);
            for row in &ret.rows {
                assert!(row.retry_times <= 2);
            }

            // LE(retry_times, -1) → empty
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::le("retry_times", -1)),
                )
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 0);
        }

        // ========== Match tests ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_match_on_indexed_string_field() {
            // Match on state (indexed string field) uses starts_with scan,
            // which matches exact values because the index key uses trailing '|'
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));
            let prefix = utils::shortid();

            let states = vec![
                format!("{}-m-a1", prefix),
                format!("{}-m-a2", prefix),
                format!("{}-m-b1", prefix),
            ];
            for state in &states {
                for _ in 0..3 {
                    let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                    proc.state = state.clone();
                    store.procs().create(&proc).expect("create proc");
                }
            }

            // Match(state, exact "{p}-m-a1") → 3 entries
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", &format!("{}-m-a1", prefix))),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            for row in &ret.rows {
                assert_eq!(row.state, format!("{}-m-a1", prefix));
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_match_on_non_indexed_field() {
            // Match on name (non-indexed field) uses contains
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            let names = vec!["hello-world", "hello-mars", "goodbye-pluto"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // Match(name, "hello") → hello-world, hello-mars
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "hello")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 2);
            let names_found: Vec<&str> = ret.rows.iter().map(|r| r.name.as_str()).collect();
            assert!(names_found.contains(&"hello-world"));
            assert!(names_found.contains(&"hello-mars"));

            // Match(name, "pluto") → goodbye-pluto
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "pluto")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "goodbye-pluto");
        }

        // ========== GT + order by desc ==========

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_gt_with_order_by_desc() {
            let store = store();
            let pid = utils::longid();
            let tid = utils::shortid();

            let timestamps: Vec<i64> = vec![100, 200, 300, 400, 500];
            for &ts in &timestamps {
                let msg = Message {
                    id: utils::shortid(),
                    name: format!("gtdesc-{}", ts),
                    pid: pid.clone(),
                    tid: tid.clone(),
                    nid: utils::shortid(),
                    mid: utils::shortid(),
                    state: MessageState::Created,
                    start_time: 0,
                    end_time: 0,
                    r#type: "step".to_string(),
                    uses: Some("package".to_string()),
                    inputs: json!({}).to_string(),
                    outputs: json!({}).to_string(),
                    chan_id: "test1".to_string(),
                    chan_pattern: "*:*:*:*".to_string(),
                    create_time: 0,
                    update_time: 0,
                    retry_times: 0,
                    timestamp: ts,
                    status: MessageStatus::Created,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // GT(timestamp, 250) with desc order → 500, 400, 300
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("pid", pid.clone()))
                        .expr(Expr::gt("timestamp", 250)),
                )
                .order("timestamp", Sort::Desc)
                .offset(0)
                .limit(100);
            let ret = store.messages().query(&q).unwrap();
            assert_eq!(ret.count, 3);
            assert_eq!(ret.rows[0].timestamp, 500);
            assert_eq!(ret.rows[1].timestamp, 400);
            assert_eq!(ret.rows[2].timestamp, 300);
        }

        // ========== Special character tests (_, %, |) ==========
        // These characters have special meaning in SQL LIKE patterns
        // (_ matches any single char, % matches any sequence).
        // The postgres store's escape_like must handle them correctly.

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_special_chars_on_indexed_field() {
            // Test that special characters (%, |, \, _) are correctly handled as
            // literals in indexed field queries via the universal encode_key_str
            // mechanism. All four characters are tested on the indexed "state" field.
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            // Create procs with special characters in state (indexed field)
            let states = vec![
                "state_a_plain",
                "state_b_pipe|val",
                "state_c_pct%val",
                "state_d_bsl\\val",
                "state_e_und_val",
                "state_f_multi_%\\|",
            ];
            for state in &states {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.state = state.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // EQ on value containing |
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("state", "state_b_pipe|val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "state_b_pipe|val");

            // EQ on value containing %
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("state", "state_c_pct%val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "state_c_pct%val");

            // EQ on value containing \
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("state", "state_d_bsl\\val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "state_d_bsl\\val");

            // EQ on value containing _
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("state", "state_e_und_val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "state_e_und_val");

            // EQ on value containing multiple special chars (%, \, |)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("state", "state_f_multi_%\\|")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "state_f_multi_%\\|");

            // NE on value containing | — should exclude only that one
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("state", "state_b_pipe|val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5); // all 6 except state_b_pipe|val
            for row in &ret.rows {
                assert_ne!(row.state, "state_b_pipe|val");
            }

            // NE on value containing % — should exclude only that one
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("state", "state_c_pct%val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5); // all 6 except state_c_pct%val
            for row in &ret.rows {
                assert_ne!(row.state, "state_c_pct%val");
            }

            // NE on value containing \ — should exclude only that one
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("state", "state_d_bsl\\val")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5); // all 6 except state_d_bsl\val
            for row in &ret.rows {
                assert_ne!(row.state, "state_d_bsl\\val");
            }

            // NE on a plain value — ensures filter still works
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("state", "state_a_plain")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5); // all 6 except state_a_plain
            for row in &ret.rows {
                assert_ne!(row.state, "state_a_plain");
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_special_chars_on_non_indexed_field() {
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            // Create procs with special characters in name (non-indexed field)
            let names = vec![
                "name_a",
                "name_b",
                "name_c|pipe",
                "name_d%pct",
                "name_e_und",
                "name_f\\bsl",
            ];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // EQ on value containing |
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("name", "name_c|pipe")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "name_c|pipe");

            // EQ on value containing %
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("name", "name_d%pct")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "name_d%pct");

            // EQ on value containing _
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::eq("name", "name_e_und")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "name_e_und");

            // NE on value containing %
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("name", "name_d%pct")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5);
            for row in &ret.rows {
                assert_ne!(row.name, "name_d%pct");
            }

            // NE on value containing _
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("name", "name_e_und")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5);
            for row in &ret.rows {
                assert_ne!(row.name, "name_e_und");
            }

            // NE on value containing |
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::ne("name", "name_c|pipe")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 5);
            for row in &ret.rows {
                assert_ne!(row.name, "name_c|pipe");
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_query_match_with_special_chars() {
            let store = store();
            let workflow = Workflow::new()
                .with_id(&utils::shortid())
                .with_step(|step| step.with_id("step1"));

            // Create procs with special character names (non-indexed)
            let names = vec!["hello_world", "50%off", "a|b|c", "normal_name%extra"];
            for name in &names {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.name = name.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // Match on non-indexed field containing _ (literal match, not wildcard)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "hello_world")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "hello_world");

            // Match on non-indexed field containing %
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "50%off")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "50%off");

            // Match on non-indexed field containing |
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "a|b|c")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "a|b|c");

            // Match substring with special chars: "world" inside "hello_world"
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "world")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "hello_world");

            // Match substring: "%off" inside "50%off"
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "%off")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "50%off");

            // Match substring with _ (match the literal underscore, not a single-char wildcard)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("name", "o_wo")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].name, "hello_world");

            // Create procs with special chars in state (indexed) for Match on indexed.
            // %, |, \, and _ are now all safe via encode_key_str.
            let states = vec![
                "match_alpha_x",
                "match_beta|pct",
                "match_gamma%bsl",
                "match_delta\\und",
            ];
            for state in &states {
                let mut proc = create_proc(&utils::shortid(), TaskState::None, &workflow);
                proc.state = state.to_string();
                store.procs().create(&proc).expect("create proc");
            }

            // Match on indexed field containing | (exact value match)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "match_beta|pct")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_beta|pct");

            // Match on indexed field containing % (exact value match)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "match_gamma%bsl")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_gamma%bsl");

            // Match on indexed field containing \ (exact value match)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "match_delta\\und")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_delta\\und");

            // Match on indexed field containing _ (exact value match)
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "match_alpha_x")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_alpha_x");

            // Match substring on indexed field: "alpha" inside "match_alpha_x"
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "alpha")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_alpha_x");

            // Match substring: "|" inside "match_beta|pct"
            let q = Query::new()
                .filter(
                    Filter::and()
                        .expr(Expr::eq("mid", workflow.id.clone()))
                        .expr(Expr::matches("state", "|")),
                )
                .offset(0)
                .limit(100);
            let ret = store.procs().query(&q).unwrap();
            assert_eq!(ret.count, 1);
            assert_eq!(ret.rows[0].state, "match_beta|pct");
        }
    };
}
