#[macro_export]
macro_rules! gen_store_tests {
    ($init:expr) => {
        use serde_json::json;
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
            }
        }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_create() {
        //     let store = store();
        //     let model = Model {
        //         id: utils::longid(),
        //         name: "test".to_string(),
        //         desc: "test desc".to_string(),
        //         ver: "0.1.0".to_string(),
        //         size: 1245,
        //         create_time: 3333,
        //         update_time: 0,
        //         data: "{}".to_string(),
        //         timestamp: 0,
        //     };
        //     store.models().create(&model).unwrap();
        //     assert!(store.models().exists(&model.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_find() {
        //     let store = store();
        //     let mid: String = utils::longid();
        //     let model = Model {
        //         id: mid.clone(),
        //         name: "test".to_string(),
        //         desc: "test desc".to_string(),
        //         ver: "0.1.0".to_string(),
        //         size: 1245,
        //         create_time: 3333,
        //         data: "{}".to_string(),
        //         update_time: 0,
        //         timestamp: 0,
        //     };
        //     store.models().create(&model).unwrap();
        //     assert_eq!(store.models().find(&mid).unwrap().id, mid);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_query_id() {
        //     let store = store();
        //     let models = store.models();
        //     for _ in 0..5 {
        //         let model = Model {
        //             id: utils::longid(),
        //             name: "test_model".to_string(),
        //             desc: "test desc".to_string(),
        //             ver: "0.1.0".to_string(),
        //             size: 1245,
        //             create_time: 3333,
        //             update_time: 0,
        //             data: "{}".to_string(),
        //             timestamp: 0,
        //         };
        //         models.create(&model).unwrap();
        //     }

        //     let q = Query::new()
        //         .filter(Filter::and().expr(Expr::eq("name", "test_model")))
        //         .limit(5);
        //     let items = models.query(&q).unwrap();
        //     assert_eq!(items.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_query_match_or() {
        //     let store = store();
        //     for i in 0..5 {
        //         let model = Model {
        //             id: utils::longid(),
        //             name: format!("test_model {i}"),
        //             desc: "test desc".to_string(),
        //             ver: "0.1.0".to_string(),
        //             size: 1000,
        //             create_time: 3333,
        //             update_time: 0,
        //             data: format!("data {i}"),
        //             timestamp: 0,
        //         };
        //         store.models().create(&model).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("size", 1000)).push(
        //             Filter::or()
        //                 .expr(Expr::matches("name", "test_model"))
        //                 .expr(Expr::matches("data", "data")),
        //         ),
        //     );

        //     let ret = store.models().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_query_match_and() {
        //     let store = store();
        //     for i in 0..5 {
        //         let model = Model {
        //             id: utils::longid(),
        //             name: format!("test_model {i}"),
        //             desc: "test desc".to_string(),
        //             ver: "0.1.0".to_string(),
        //             size: 2000,
        //             create_time: 3333,
        //             update_time: 0,
        //             data: format!("data {i}"),
        //             timestamp: 0,
        //         };
        //         store.models().create(&model).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("size", 2000)).push(
        //             Filter::and()
        //                 .expr(Expr::matches("name", "0"))
        //                 .expr(Expr::matches("data", "0")),
        //         ),
        //     );

        //     let ret = store.models().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_update() {
        //     let store = store();

        //     let mut model = Model {
        //         id: utils::longid(),
        //         name: "test".to_string(),
        //         desc: "test desc".to_string(),
        //         ver: "0.1.0".to_string(),
        //         size: 1245,
        //         create_time: 3333,
        //         update_time: 0,
        //         data: "{}".to_string(),
        //         timestamp: 0,
        //     };
        //     store.models().create(&model).unwrap();

        //     model.ver = "0.2.0".to_string();
        //     model.update_time = 1;
        //     store.models().update(&model).unwrap();

        //     let p = store.models().find(&model.id).unwrap();
        //     assert_eq!(p.ver, model.ver);
        //     assert!(p.update_time > 0);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_model_delete() {
        //     let store = store();
        //     let model = Model {
        //         id: utils::longid(),
        //         name: "test".to_string(),
        //         desc: "test desc".to_string(),
        //         ver: "0.1.0".to_string(),
        //         size: 1245,
        //         create_time: 3333,
        //         update_time: 0,
        //         data: "{}".to_string(),
        //         timestamp: 0,
        //     };
        //     store.models().create(&model).unwrap();
        //     store.models().delete(&model.id).unwrap();

        //     assert!(!store.procs().exists(&model.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_create() {
        //     let store = store();
        //     let proc = Proc {
        //         id: utils::longid(),
        //         name: "name".to_string(),
        //         mid: "m1".to_string(),
        //         state: TaskState::None.into(),
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         model: "".to_string(),
        //         env: "".to_string(),
        //         err: None,
        //     };
        //     store.procs().create(&proc).unwrap();
        //     assert!(store.procs().exists(&proc.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_find() {
        //     let store = store();
        //     let pid = utils::longid();
        //     let proc = Proc {
        //         id: pid.clone(),
        //         name: "name".to_string(),
        //         mid: "m1".to_string(),
        //         state: TaskState::None.into(),
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         model: "".to_string(),
        //         env: "".to_string(),
        //         err: None,
        //     };
        //     store.procs().create(&proc).unwrap();
        //     assert_eq!(store.procs().find(&pid).unwrap().id, pid);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_query_id() {
        //     let store = store();
        //     let procs = store.procs();
        //     let mid = utils::longid();
        //     for i in 0..5 {
        //         let proc = Proc {
        //             id: utils::longid(),
        //             name: i.to_string(),
        //             mid: mid.to_string(),
        //             state: TaskState::None.into(),
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             model: "".to_string(),
        //             env: "".to_string(),
        //             err: None,
        //         };
        //         procs.create(&proc).unwrap();
        //     }

        //     let q = Query::new()
        //         .filter(Filter::and().expr(Expr::eq("mid", mid)))
        //         .limit(5);
        //     let items = procs.query(&q).unwrap();
        //     assert_eq!(items.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_query_match_or() {
        //     let store = store();
        //     let mid = utils::longid();
        //     for i in 0..5 {
        //         let proc = Proc {
        //             id: utils::longid(),
        //             name: format!("name {i}"),
        //             mid: mid.to_string(),
        //             state: TaskState::None.into(),
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             model: format!("model {i}"),
        //             env: "".to_string(),
        //             err: None,
        //         };
        //         store.procs().create(&proc).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("mid", mid)).push(
        //             Filter::or()
        //                 .expr(Expr::matches("name", "name"))
        //                 .expr(Expr::matches("model", "model")),
        //         ),
        //     );

        //     let ret = store.procs().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_query_match_and() {
        //     let store = store();
        //     let mid = utils::longid();
        //     for i in 0..5 {
        //         let proc = Proc {
        //             id: utils::longid(),
        //             name: format!("name {i}"),
        //             mid: mid.to_string(),
        //             state: TaskState::None.into(),
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             model: format!("model {i}"),
        //             env: "".to_string(),
        //             err: None,
        //         };
        //         store.procs().create(&proc).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("mid", mid)).push(
        //             Filter::and()
        //                 .expr(Expr::matches("name", "0"))
        //                 .expr(Expr::matches("model", "0")),
        //         ),
        //     );

        //     let ret = store.procs().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_update() {
        //     let store = store();

        //     let mut vars: Vars = Vars::new();
        //     vars.insert("k1".to_string(), "v1".into());

        //     let mut proc = Proc {
        //         id: utils::shortid(),
        //         name: "test".to_string(),
        //         mid: "m1".to_string(),
        //         state: TaskState::None.into(),
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         model: "".to_string(),
        //         env: "".to_string(),
        //         err: None,
        //     };
        //     store.procs().create(&proc).unwrap();

        //     proc.state = TaskState::Running.into();
        //     store.procs().update(&proc).unwrap();

        //     let p = store.procs().find(&proc.id).unwrap();
        //     assert_eq!(p.state, proc.state);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_proc_delete() {
        //     let store = store();
        //     let proc = Proc {
        //         id: utils::shortid(),
        //         name: "test".to_string(),
        //         mid: "m1".to_string(),
        //         state: TaskState::None.into(),
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         model: "".to_string(),
        //         env: "".to_string(),
        //         err: None,
        //     };
        //     store.procs().create(&proc).unwrap();
        //     store.procs().delete(&proc.id).unwrap();

        //     assert!(!store.procs().exists(&proc.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_create() {
        //     let store = store();
        //     let tasks = store.tasks();
        //     let task = Task {
        //         id: utils::shortid(),
        //         kind: NodeKind::Workflow.into(),
        //         name: "test".to_string(),
        //         pid: "pid".to_string(),
        //         tid: "tid".to_string(),
        //         node_data: "nid".to_string(),
        //         state: TaskState::None.into(),
        //         prev: None,
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         data: "{}".to_string(),
        //         err: None,
        //     };
        //     tasks.create(&task).unwrap();
        //     assert!(tasks.exists(&task.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_find() {
        //     let store = store();
        //     let tasks = store.tasks();
        //     let tid = utils::shortid();
        //     let task = Task {
        //         id: tid.clone(),
        //         kind: NodeKind::Workflow.into(),
        //         name: "test".to_string(),
        //         pid: "pid".to_string(),
        //         tid: "tid".to_string(),
        //         node_data: "nid".to_string(),
        //         state: TaskState::None.into(),
        //         data: "{}".to_string(),
        //         prev: None,
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         err: None,
        //     };
        //     tasks.create(&task).unwrap();
        //     assert_eq!(tasks.find(&tid).unwrap().id, tid);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_query_id() {
        //     let store = store();
        //     let tasks = store.tasks();
        //     let pid = utils::shortid();
        //     for _ in 0..5 {
        //         let task = Task {
        //             kind: NodeKind::Workflow.into(),
        //             id: utils::shortid(),
        //             name: "test".to_string(),
        //             pid: pid.to_string(),
        //             tid: "tid".to_string(),
        //             node_data: "nid".to_string(),
        //             state: TaskState::None.into(),
        //             prev: None,
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             data: "{}".to_string(),
        //             err: None,
        //         };
        //         tasks.create(&task).unwrap();
        //     }

        //     let q = Query::new()
        //         .filter(Filter::and().expr(Expr::eq("pid", pid)))
        //         .limit(5);
        //     let items = tasks.query(&q).unwrap();
        //     assert_eq!(items.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_query_match_or() {
        //     let store = store();
        //     let pid = utils::shortid();
        //     for idx in 0..5 {
        //         let task = Task {
        //             kind: NodeKind::Workflow.into(),
        //             id: utils::shortid(),
        //             name: format!("test {idx}"),
        //             tid: format!("tid {idx}"),
        //             pid: pid.to_string(),
        //             node_data: "nid2".to_string(),
        //             state: TaskState::None.into(),
        //             prev: None,
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             data: "{}".to_string(),
        //             err: None,
        //         };
        //         store.tasks().create(&task).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("node_data", "nid2")).push(
        //             Filter::or()
        //                 .expr(Expr::matches("name", "test"))
        //                 .expr(Expr::matches("tid", "tid")),
        //         ),
        //     );

        //     let ret = store.tasks().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_query_match_and() {
        //     let store = store();

        //     let pid = utils::shortid();
        //     for idx in 0..5 {
        //         let task = Task {
        //             kind: NodeKind::Workflow.into(),
        //             id: utils::shortid(),
        //             name: format!("test {idx}"),
        //             tid: format!("tid {idx}"),
        //             pid: pid.to_string(),
        //             node_data: "nid3".to_string(),
        //             state: TaskState::None.into(),
        //             prev: None,
        //             start_time: 0,
        //             end_time: 0,
        //             timestamp: 0,
        //             data: "{}".to_string(),
        //             err: None,
        //         };
        //         store.tasks().create(&task).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("node_data", "nid3")).push(
        //             Filter::and()
        //                 .expr(Expr::matches("name", "0"))
        //                 .expr(Expr::matches("tid", "0")),
        //         ),
        //     );

        //     let ret = store.tasks().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_update() {
        //     let store = store();
        //     let table = store.tasks();
        //     let mut task = Task {
        //         kind: NodeKind::Workflow.into(),
        //         id: utils::shortid(),
        //         name: "test".to_string(),
        //         pid: "pid".to_string(),
        //         tid: "tid".to_string(),
        //         node_data: "nid".to_string(),
        //         state: TaskState::None.into(),
        //         prev: None,
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         data: "{}".to_string(),
        //         err: None,
        //     };
        //     table.create(&task).unwrap();

        //     task.state = TaskState::Completed.into();
        //     task.prev = Some("tid1".to_string());
        //     table.update(&task).unwrap();

        //     let t = table.find(&task.id).unwrap();
        //     assert_eq!(t.state, task.state);
        //     assert_eq!(t.prev, task.prev);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_task_delete() {
        //     let store = store();
        //     let table = store.tasks();
        //     let task = Task {
        //         kind: NodeKind::Workflow.into(),
        //         id: utils::shortid(),
        //         name: "test".to_string(),
        //         pid: "pid".to_string(),
        //         tid: "tid".to_string(),
        //         node_data: "nid".to_string(),
        //         state: TaskState::None.into(),
        //         prev: None,
        //         start_time: 0,
        //         end_time: 0,
        //         timestamp: 0,
        //         data: "{}".to_string(),
        //         err: None,
        //     };
        //     table.create(&task).unwrap();
        //     table.delete(&task.id).unwrap();

        //     assert!(!table.exists(&task.id).unwrap());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_create() {
        //     let store = store();

        //     let pid = utils::longid();
        //     let tid = utils::shortid();
        //     let msg = Message {
        //         id: format!("{pid}:{tid}"),
        //         name: "test".to_string(),
        //         pid: pid.clone(),
        //         tid: tid.clone(),
        //         nid: utils::shortid(),
        //         mid: utils::shortid(),
        //         state: MessageState::Created,
        //         start_time: 0,
        //         end_time: 0,
        //         r#type: "step".to_string(),
        //         key: "test".to_string(),
        //         uses: "package".to_string(),
        //         inputs: json!({}).to_string(),
        //         outputs: json!({}).to_string(),
        //         tag: "tag1".to_string(),
        //         chan_id: "test1".to_string(),
        //         chan_pattern: "*:*:*:*".to_string(),
        //         create_time: 0,
        //         update_time: 0,
        //         retry_times: 0,
        //         timestamp: 0,
        //         status: MessageStatus::Created,
        //     };

        //     store.messages().create(&msg).expect("create message");

        //     let id = utils::Id::new(&pid, &tid);
        //     let ret = store.messages().find(&id.id());
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_query_id() {
        //     let store = store();

        //     let pid = utils::longid();
        //     let tid = utils::shortid();
        //     let msg = Message {
        //         id: format!("{pid}:{tid}"),
        //         name: "test".to_string(),
        //         pid: pid.clone(),
        //         tid: tid.clone(),
        //         nid: utils::shortid(),
        //         mid: utils::shortid(),
        //         state: MessageState::Created,
        //         start_time: 0,
        //         end_time: 0,
        //         r#type: "step".to_string(),
        //         key: "test".to_string(),
        //         uses: "package".to_string(),
        //         inputs: json!({}).to_string(),
        //         outputs: json!({}).to_string(),
        //         tag: "tag1".to_string(),
        //         chan_id: "test1".to_string(),
        //         chan_pattern: "*:*:*:*".to_string(),
        //         create_time: 0,
        //         update_time: 0,
        //         retry_times: 0,
        //         timestamp: 0,
        //         status: MessageStatus::Created,
        //     };

        //     store.messages().create(&msg).expect("create message");

        //     let id = utils::Id::new(&pid, &tid);
        //     let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
        //     let ret = store.messages().query(&q);
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_query_match_or() {
        //     let store = store();

        //     let chan_id = utils::shortid();
        //     for idx in 0..5 {
        //         let pid = utils::longid();
        //         let tid = utils::shortid();
        //         let msg = Message {
        //             id: format!("{pid}:{tid}"),
        //             name: "test".to_string(),
        //             pid: pid.clone(),
        //             tid: tid.clone(),
        //             nid: utils::shortid(),
        //             mid: utils::shortid(),
        //             state: MessageState::Created,
        //             start_time: 0,
        //             end_time: 0,
        //             r#type: "step".to_string(),
        //             key: format!("test {idx}"),
        //             uses: format!("package {idx}"),
        //             inputs: json!({}).to_string(),
        //             outputs: json!({}).to_string(),
        //             tag: "tag1".to_string(),
        //             chan_id: chan_id.clone(),
        //             chan_pattern: "*:*:*:*".to_string(),
        //             create_time: 0,
        //             update_time: 0,
        //             retry_times: 0,
        //             timestamp: 0,
        //             status: MessageStatus::Created,
        //         };

        //         store.messages().create(&msg).expect("create message");
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("chan_id", chan_id)).push(
        //             Filter::or()
        //                 .expr(Expr::matches("key", "test"))
        //                 .expr(Expr::matches("uses", "package")),
        //         ),
        //     );

        //     let ret = store.messages().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_query_match_and() {
        //     let store = store();
        //     let chan_id = utils::shortid();
        //     for idx in 0..5 {
        //         let pid = utils::longid();
        //         let tid = utils::shortid();
        //         let msg = Message {
        //             id: format!("{pid}:{tid}"),
        //             name: "test".to_string(),
        //             pid: pid.clone(),
        //             tid: tid.clone(),
        //             nid: utils::shortid(),
        //             mid: utils::shortid(),
        //             state: MessageState::Created,
        //             start_time: 0,
        //             end_time: 0,
        //             r#type: "step".to_string(),
        //             key: format!("test {idx}"),
        //             uses: format!("package {idx}"),
        //             inputs: json!({}).to_string(),
        //             outputs: json!({}).to_string(),
        //             tag: "tag1".to_string(),
        //             chan_id: chan_id.clone(),
        //             chan_pattern: "*:*:*:*".to_string(),
        //             create_time: 0,
        //             update_time: 0,
        //             retry_times: 0,
        //             timestamp: 0,
        //             status: MessageStatus::Created,
        //         };

        //         store.messages().create(&msg).expect("create message");
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("chan_id", chan_id)).push(
        //             Filter::and()
        //                 .expr(Expr::matches("key", "0"))
        //                 .expr(Expr::matches("uses", "0")),
        //         ),
        //     );

        //     let ret = store.messages().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_update() {
        //     let store = store();

        //     let pid = utils::longid();
        //     let tid = utils::shortid();
        //     let msg = Message {
        //         id: format!("{pid}:{tid}"),
        //         name: "test".to_string(),
        //         pid: pid.clone(),
        //         tid: tid.clone(),
        //         nid: utils::shortid(),
        //         mid: utils::shortid(),
        //         state: MessageState::Created,
        //         start_time: 0,
        //         end_time: 0,
        //         r#type: "step".to_string(),
        //         key: "test".to_string(),
        //         uses: "package".to_string(),
        //         inputs: json!({}).to_string(),
        //         outputs: json!({}).to_string(),
        //         tag: "tag1".to_string(),
        //         chan_id: "test1".to_string(),
        //         chan_pattern: "*:*:*:*".to_string(),
        //         create_time: 0,
        //         update_time: 0,
        //         retry_times: 0,
        //         timestamp: 0,
        //         status: MessageStatus::Created,
        //     };

        //     store.messages().create(&msg).unwrap();

        //     let id = utils::Id::new(&pid, &tid);
        //     let mut msg = store.messages().find(&id.id()).unwrap();
        //     msg.state = MessageState::Completed;
        //     msg.retry_times = 1;
        //     msg.status = MessageStatus::Acked;
        //     store.messages().update(&msg).unwrap();

        //     let msg2 = store.messages().find(&id.id()).unwrap();
        //     assert_eq!(msg2.state, MessageState::Completed);
        //     assert_eq!(msg2.retry_times, 1);
        //     assert_eq!(msg2.status, MessageStatus::Acked);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_message_remove() {
        //     let store = store();

        //     let pid = utils::longid();
        //     let tid = utils::shortid();
        //     let msg = Message {
        //         id: format!("{pid}:{tid}"),
        //         name: "test".to_string(),
        //         pid: pid.clone(),
        //         tid: tid.clone(),
        //         nid: utils::shortid(),
        //         mid: utils::shortid(),
        //         state: MessageState::Created,
        //         start_time: 0,
        //         end_time: 0,
        //         r#type: "step".to_string(),
        //         key: "test".to_string(),
        //         uses: "package".to_string(),
        //         inputs: json!({}).to_string(),
        //         outputs: json!({}).to_string(),
        //         tag: "tag1".to_string(),
        //         chan_id: "test1".to_string(),
        //         chan_pattern: "*:*:*:*".to_string(),
        //         create_time: 0,
        //         update_time: 0,
        //         retry_times: 0,
        //         timestamp: 0,
        //         status: MessageStatus::Created,
        //     };

        //     store.messages().create(&msg).unwrap();
        //     store.messages().delete(&msg.id).unwrap();

        //     let ret = store.messages().find(&msg.id);
        //     assert!(ret.is_err());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_create() {
        //     let store = store();

        //     let id = utils::longid();
        //     let evt = Event {
        //         id,
        //         name: "name".to_string(),
        //         mid: "mid".to_string(),
        //         ver: "0.1.0".to_string(),
        //         uses: "acts.event.manual".to_string(),
        //         params: "".to_string(),
        //         create_time: utils::time::time_millis(),
        //         timestamp: utils::time::timestamp(),
        //     };

        //     store.events().create(&evt).unwrap();
        //     let ret = store.events().find(&evt.id);
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_query_id() {
        //     let store = store();

        //     let id = utils::longid();
        //     let evt = Event {
        //         id,
        //         name: "name".to_string(),
        //         mid: "mid".to_string(),
        //         ver: "0.1.0".to_string(),
        //         uses: "acts.event.manual".to_string(),
        //         params: "".to_string(),
        //         create_time: 0,
        //         timestamp: 0,
        //     };
        //     store.events().create(&evt).unwrap();
        //     let q = Query::new().filter(Filter::and().expr(Expr::eq("id", evt.id)));
        //     let ret = store.events().query(&q);
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_query_match_or() {
        //     let store = store();

        //     for idx in 0..5 {
        //         let id = utils::longid();
        //         let evt = Event {
        //             id,
        //             name: format!("name {idx}"),
        //             mid: "mid1".to_string(),
        //             ver: "0.1.0".to_string(),
        //             uses: "acts.event.manual".to_string(),
        //             params: "".to_string(),
        //             create_time: 0,
        //             timestamp: 0,
        //         };
        //         store.events().create(&evt).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("mid", "mid1")).push(
        //             Filter::or()
        //                 .expr(Expr::matches("name", "name"))
        //                 .expr(Expr::matches("uses", "manual")),
        //         ),
        //     );

        //     let ret = store.events().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_query_match_and() {
        //     let store = store();

        //     for idx in 0..5 {
        //         let id = utils::longid();
        //         let evt = Event {
        //             id,
        //             name: format!("name {idx}"),
        //             mid: "mid2".to_string(),
        //             ver: "0.1.0".to_string(),
        //             uses: "acts.event.manual".to_string(),
        //             params: "".to_string(),
        //             create_time: 0,
        //             timestamp: 0,
        //         };
        //         store.events().create(&evt).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("mid", "mid2")).push(
        //             Filter::and()
        //                 .expr(Expr::matches("name", "0"))
        //                 .expr(Expr::matches("uses", "manual")),
        //         ),
        //     );

        //     let ret = store.events().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_update() {
        //     let store = store();

        //     let id = utils::longid();
        //     let evt = Event {
        //         id,
        //         name: "name".to_string(),
        //         mid: "mid".to_string(),
        //         ver: "0.1.0".to_string(),
        //         uses: "acts.event.manual".to_string(),
        //         params: "".to_string(),
        //         create_time: 0,
        //         timestamp: 0,
        //     };
        //     store.events().create(&evt).unwrap();
        //     let mut p = store.events().find(&evt.id).unwrap();
        //     p.name = "my name".to_string();
        //     p.timestamp = 200;
        //     p.mid = "my mid".to_string();

        //     store.events().update(&p).unwrap();

        //     let p2 = store.events().find(&evt.id).unwrap();
        //     assert_eq!(p2.name, "my name");
        //     assert_eq!(p2.timestamp, 200);
        //     assert_eq!(p2.mid, "my mid");
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_event_remove() {
        //     let store = store();

        //     let id = utils::longid();
        //     let evt = Event {
        //         id,
        //         name: "name".to_string(),
        //         mid: "mid".to_string(),
        //         ver: "0.1.0".to_string(),
        //         uses: "acts.event.manual".to_string(),
        //         params: "".to_string(),
        //         create_time: 0,
        //         timestamp: 0,
        //     };
        //     store.events().create(&evt).unwrap();
        //     store.events().delete(&evt.id).unwrap();

        //     let ret = store.events().find(&evt.id);
        //     assert!(ret.is_err());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_create() {
        //     let store = store();

        //     let id = utils::longid();
        //     let package = Package {
        //         id,
        //         name: "name".to_string(),
        //         desc: "desc".to_string(),
        //         icon: "icon".to_string(),
        //         doc: "doc".to_string(),
        //         version: "0.1.0".to_string(),
        //         in_schema: "{}".to_string(),
        //         ui_schema: None,
        //         run_as: $crate::ActRunAs::Func,
        //         resources: "[]".to_string(),
        //         catalog: $crate::ActPackageCatalog::Core,
        //         create_time: 0,
        //         update_time: 0,
        //         timestamp: 0,
        //         built_in: false,
        //     };

        //     store.packages().create(&package).unwrap();
        //     let ret = store.packages().find(&package.id);
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_query_id() {
        //     let store = store();

        //     let id = utils::longid();
        //     let package = Package {
        //         id,
        //         name: "name".to_string(),
        //         desc: "desc".to_string(),
        //         icon: "icon".to_string(),
        //         doc: "doc".to_string(),
        //         version: "0.1.0".to_string(),
        //         in_schema: "{}".to_string(),
        //         ui_schema: None,
        //         run_as: $crate::ActRunAs::Func,
        //         resources: "[]".to_string(),
        //         catalog: $crate::ActPackageCatalog::Core,
        //         create_time: 0,
        //         update_time: 0,
        //         timestamp: 0,
        //         built_in: false,
        //     };
        //     store.packages().create(&package).unwrap();
        //     let q = Query::new().filter(Filter::and().expr(Expr::eq("id", package.id)));
        //     let ret = store.packages().query(&q);
        //     assert!(ret.is_ok());
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_query_match_or() {
        //     let store = store();
        //     let name = utils::shortid();
        //     for idx in 0..5 {
        //         let id = utils::longid();
        //         let package = Package {
        //             id,
        //             name: name.clone(),
        //             desc: format!("desc text {idx}"),
        //             icon: format!("icon text {idx}"),
        //             doc: "doc".to_string(),
        //             version: "0.2.0".to_string(),
        //             in_schema: "{}".to_string(),
        //             ui_schema: None,
        //             run_as: $crate::ActRunAs::Func,
        //             resources: "[]".to_string(),
        //             catalog: $crate::ActPackageCatalog::Core,
        //             create_time: 0,
        //             update_time: 0,
        //             timestamp: 0,
        //             built_in: false,
        //         };
        //         store.packages().create(&package).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("name", name)).push(
        //             Filter::or()
        //                 .expr(Expr::matches("desc", "desc"))
        //                 .expr(Expr::matches("icon", "icon")),
        //         ),
        //     );

        //     let ret = store.packages().query(&q).unwrap();
        //     assert_eq!(ret.count, 5);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_query_match_and() {
        //     let store = store();
        //     let name = utils::shortid();
        //     for idx in 0..5 {
        //         let id = utils::longid();
        //         let package = Package {
        //             id,
        //             name: name.clone(),
        //             desc: format!("desc text {idx}"),
        //             icon: format!("icon text {idx}"),
        //             doc: "doc".to_string(),
        //             version: "0.3.0".to_string(),
        //             in_schema: "{}".to_string(),
        //             ui_schema: None,
        //             run_as: $crate::ActRunAs::Func,
        //             resources: "[]".to_string(),
        //             catalog: $crate::ActPackageCatalog::Core,
        //             create_time: 0,
        //             update_time: 0,
        //             timestamp: 0,
        //             built_in: false,
        //         };
        //         store.packages().create(&package).unwrap();
        //     }

        //     let q = Query::new().filter(
        //         Filter::and().expr(Expr::eq("name", name)).push(
        //             Filter::and()
        //                 .expr(Expr::matches("desc", "0"))
        //                 .expr(Expr::matches("icon", "0")),
        //         ),
        //     );

        //     let ret = store.packages().query(&q).unwrap();
        //     assert_eq!(ret.count, 1);
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_update() {
        //     let store = store();

        //     let id = utils::longid();
        //     let package = Package {
        //         id,
        //         name: "test name".to_string(),
        //         desc: "desc".to_string(),
        //         icon: "icon".to_string(),
        //         doc: "doc".to_string(),
        //         version: "0.1.0".to_string(),
        //         in_schema: "{}".to_string(),
        //         ui_schema: None,
        //         run_as: $crate::ActRunAs::Func,
        //         resources: "[]".to_string(),
        //         catalog: $crate::ActPackageCatalog::Core,
        //         create_time: 0,
        //         update_time: 0,
        //         timestamp: 0,
        //         built_in: false,
        //     };
        //     store.packages().create(&package).unwrap();
        //     let mut p = store.packages().find(&package.id).unwrap();
        //     p.desc = "my desc".to_string();
        //     p.version = "0.2.0-updated".to_string();
        //     p.in_schema = "{ 'b': 100 }".to_string();
        //     store.packages().update(&p).unwrap();

        //     let p2 = store.packages().find(&package.id).unwrap();
        //     assert_eq!(p2.desc, "my desc");
        //     assert_eq!(p2.version, "0.2.0-updated");
        //     assert_eq!(p2.in_schema, "{ 'b': 100 }");
        // }

        // #[tokio::test(flavor = "multi_thread")]
        // async fn store_mem_package_remove() {
        //     let store = store();

        //     let id = utils::longid();
        //     let package = Package {
        //         id,
        //         name: "test name".to_string(),
        //         desc: "desc".to_string(),
        //         icon: "icon".to_string(),
        //         doc: "doc".to_string(),
        //         version: "0.1.0".to_string(),
        //         in_schema: "{}".to_string(),
        //         ui_schema: None,
        //         run_as: $crate::ActRunAs::Func,
        //         resources: "[]".to_string(),
        //         catalog: $crate::ActPackageCatalog::Core,
        //         create_time: 0,
        //         update_time: 0,
        //         timestamp: 0,
        //         built_in: false,
        //     };
        //     store.packages().create(&package).unwrap();
        //     store.packages().delete(&package.id).unwrap();

        //     let ret = store.packages().find(&package.id);
        //     assert!(ret.is_err());
        // }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_model_deploy_ok() {
            let store = store();
            let workflow = create_workflow();
            let ok = store.deploy(&workflow).unwrap();
            assert!(ok);
        }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_model_get() {
            let store = store();
            let mut workflow = create_workflow();
            workflow.id = utils::longid();
            store.deploy(&workflow).unwrap();

            let model = store.models().find(&workflow.id).unwrap();
            assert_eq!(model.id, workflow.id);
        }

        #[tokio::test(flavor = "multi_thread")]
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
            };
            store.models().create(&model).expect("create model");
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", model.id)));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_model_deploy_id_error() {
            let store = store();
            let mut workflow = create_workflow();
            workflow.id = "".to_string();
            let result = store.deploy(&workflow);

            assert!(result.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
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
            };

            store.procs().create(&proc).expect("create process");
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", proc.id)));
            let ret = store.procs().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_task_create() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}_{tid}"),
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
            };

            store.tasks().create(&task).expect("create task");

            let id = utils::Id::new(&pid, &tid);
            let ret = store.tasks().find(&id.id());
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn store_task_query_by_id() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let task = Task {
                id: format!("{pid}_{tid}"),
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
            };

            store.tasks().create(&task).expect("create task");

            let id = utils::Id::new(&pid, &tid);
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_task_update() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}_{tid}"),
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
        async fn store_task_remove() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let nid = utils::shortid();
            let task = Task {
                id: format!("{pid}_{tid}"),
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
            };

            store.tasks().create(&task).expect("create task");
            store.tasks().delete(&task.id).expect("remove process");

            let ret = store.tasks().find(&task.id);
            assert!(ret.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn store_message_create() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}_{tid}"),
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
            };

            store.messages().create(&msg).expect("create message");

            let id = utils::Id::new(&pid, &tid);
            let ret = store.messages().find(&id.id());
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn store_message_query_by_id() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}_{tid}"),
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
            };

            store.messages().create(&msg).unwrap();

            let id = utils::Id::new(&pid, &tid);
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", id.id())));
            let ret = store.messages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
        async fn store_message_update() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}_{tid}"),
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
        async fn store_message_remove() {
            let store = store();

            let pid = utils::longid();
            let tid = utils::shortid();
            let msg = Message {
                id: format!("{pid}_{tid}"),
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
            };

            store.messages().create(&msg).unwrap();
            store.messages().delete(&msg.id).unwrap();

            let ret = store.messages().find(&msg.id);
            assert!(ret.is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
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
            };

            store.packages().create(&package).unwrap();
            let ret = store.packages().find(&package.id);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
            };
            store.packages().create(&package).unwrap();
            let q = Query::new().filter(Filter::and().expr(Expr::eq("id", package.id)));
            let ret = store.packages().query(&q);
            assert!(ret.is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
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
            };
            store.packages().create(&package).unwrap();
            store.packages().delete(&package.id).unwrap();

            let ret = store.packages().find(&package.id);
            assert!(ret.is_err());
        }
    };
}
