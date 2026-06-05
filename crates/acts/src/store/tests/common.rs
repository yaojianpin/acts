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
            let ok = store.deploy(&workflow).unwrap();
            assert!(ok);
        }

        #[tokio::test(flavor = "multi_thread")]
        #[serial(store_tests)]
        async fn store_models() {
            let store = store();

            let mut workflow = create_workflow();
            workflow.id = utils::longid();
            store.deploy(&workflow).unwrap();

            workflow.id = utils::longid();
            store.deploy(&workflow).unwrap();

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
            store.deploy(&workflow).unwrap();

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
            store.deploy(&workflow).unwrap();

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
            let result = store.deploy(&workflow);

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
                in_schema: "{}".to_string(),
                ui_schema: None,
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
                in_schema: "{}".to_string(),
                ui_schema: None,
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
                    in_schema: "{}".to_string(),
                    ui_schema: None,
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
                    in_schema: "{}".to_string(),
                    ui_schema: None,
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
                    in_schema: "{}".to_string(),
                    ui_schema: None,
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
                    in_schema: "{}".to_string(),
                    ui_schema: None,
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
                    in_schema: "{}".to_string(),
                    ui_schema: None,
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
                in_schema: "{}".to_string(),
                ui_schema: None,
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
                in_schema: "{}".to_string(),
                ui_schema: None,
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
                v: 0,
            };
            store.messages().create(&msg).expect("create message");

            // find goes through upcast
            let found = store.messages().find(&msg.id).unwrap();
            assert_eq!(found.id, msg.id);
            assert_eq!(found.v, 0);

            // query goes through upcast
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("id", msg.id.clone())))
                .limit(1);
            let page = store.messages().query(&q).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(page.rows[0].v, 0);
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
                in_schema: "{}".to_string(),
                ui_schema: None,
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
                    status,
                    v: 0,
                };
                store.messages().create(&msg).unwrap();
            }

            // name is NOT indexed, status IS indexed.
            // The fix should match "status" (indexed) not "name" (unindexed).
            let q = Query::new()
                .filter(Filter::and().expr(Expr::eq("pid", pid.clone())))
                .order("name", Sort::Asc)   // NOT indexed — should be skipped
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
    };
}
