use crate::{
    sch::NodeKind,
    store::{data, DbSet, Store, StoreKind},
    utils, Query, StoreAdapter, TaskState, Workflow,
};
use data::{Message, Proc, Task};
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
    let store = Store::new();

    #[cfg(feature = "store")]
    assert_eq!(store.kind(), StoreKind::Local);

    #[cfg(feature = "sqlite")]
    assert_eq!(store.kind(), StoreKind::Sqlite);
}

// #[tokio::test]
//  fn store_extern() {
//     let engine = Engine::new();

//     let test_store = TestStore;
//     engine.adapter().set_store_adapter("store", test_store);
//     let store = store(&engine).await;

//     assert_eq!(store.kind(), StoreKind::Extern);
// }

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
    let workflow = create_workflow();
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

// #[tokio::test]
//  fn store_task_update() {
//     let engine = Engine::new();
//     engine.start();

//     let store = engine.store();
//     let store2 = store.clone();
//     engine
//         .emitter()
//         .on_task(move |task: &Task, data: &EventData| {
//             println!(
//                 "update task:{}, kind={} data={:?}",
//                 task.tid,
//                 task.node.kind(),
//                 data
//             );
//             if data.action == EventAction::Create {
//                 store.create_task(task);
//             } else {
//                 store.update_task(task, &data.vars);
//             }
//         });

//     let scher = engine.scher();

//     let id = utils::longid();
//     let workflow = create_workflow();
//     let proc = engine.scher().create_raw_proc(&id, &workflow);
//     scher.cache().push(&proc);

//     // proc.start();
//     scher.sched_proc(&proc);
//     // proc.start();
//     scher.next().await;
//     scher.next().await;
//     scher.next().await;
//     // scher.next().await;

//     let tasks = proc.task_by_nid("job1");
//     let job = tasks.get(0).unwrap();
//     std::thread::sleep(std::time::Duration::from_millis(200));

//     let p = store2.proc(&proc.pid(), &engine.scher()).unwrap();

//     let tasks = p.task_by_nid("job1");
//     let loaded_job = tasks.get(0).unwrap();
//     assert_eq!(job.state(), TaskState::Running);
//     assert_eq!(loaded_job.state(), TaskState::Running);
// }

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
        uid: "".to_string(),
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
        uid: "".to_string(),
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
        uid: "".to_string(),
    };

    store.tasks().create(&task).expect("create task");
    store.tasks().delete(&task.id).expect("remove proc");

    let ret = store.tasks().find(&task.id);
    assert!(ret.is_err());
}

#[tokio::test]
async fn store_message_create() {
    let store = store().await;

    let id = utils::longid();
    let time = utils::time::time();
    let msg = Message {
        id: id.clone(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "Tom".to_string(),
        vars: "{}".to_string(),
        create_time: time,
        update_time: 0,
        state: 0,
    };

    store.messages().create(&msg).expect("create message");
    let ret = store.messages().find(&id).unwrap();
    assert_eq!(ret.uid, "Tom");
    assert_eq!(ret.vars, "{}");
    assert_eq!(ret.create_time, time);
}

#[tokio::test]
async fn store_message_update() {
    let store = store().await;

    let id = utils::longid();
    let mut msg = Message {
        id: id.clone(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "Tom".to_string(),
        vars: "".to_string(),
        create_time: 0,
        update_time: 0,
        state: 0,
    };

    store.messages().create(&msg).expect("create message");
    msg.uid = "Job".to_string();
    msg.vars = "vars".to_string();
    msg.update_time = 1000;
    let ret = store.messages().update(&msg);
    let new_msg = store.messages().find(&id).unwrap();
    assert!(ret.is_ok());
    assert_eq!(new_msg.uid, "Job");
    assert_eq!(new_msg.vars, "vars");
    assert_eq!(new_msg.update_time, 1000);
}

#[tokio::test]
async fn store_message_delete() {
    let store = store().await;

    let id = utils::longid();
    let mut msg = Message {
        id: id.clone(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        uid: "Tom".to_string(),
        vars: "".to_string(),
        create_time: 0,
        update_time: 0,
        state: 0,
    };

    store.messages().create(&msg).expect("create message");
    msg.uid = "Job".to_string();
    msg.vars = "vars".to_string();
    msg.update_time = 1000;
    let ret = store.messages().delete(&id);
    assert!(ret.is_ok());
    let ret = store.messages().find(&id);
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

    fn messages(&self) -> Arc<dyn DbSet<Item = data::Message>> {
        todo!()
    }

    fn init(&self) {}

    fn flush(&self) {}
}
