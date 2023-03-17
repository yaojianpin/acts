use crate::{
    debug,
    sch::{EventAction, EventData, Task},
    store::{data, DataSet, Store, StoreKind},
    utils, Engine, StoreAdapter, TaskState, Workflow,
};
use std::sync::Arc;
use tokio::sync::OnceCell;

static STORE: OnceCell<Store> = OnceCell::const_new();
async fn init() -> Store {
    let s = Store::new();
    s
}

async fn store(engine: &Engine) -> &'static Store {
    let store = STORE.get_or_init(init).await;
    store.init(&engine);

    store
}

#[tokio::test]
async fn store_local() {
    let engine = Engine::new();
    let store = store(&engine).await;

    #[cfg(feature = "store")]
    assert_eq!(store.kind(), StoreKind::Local);

    #[cfg(feature = "sqlite")]
    assert_eq!(store.kind(), StoreKind::Sqlite);
}

// #[tokio::test]
// async fn store_extern() {
//     let engine = Engine::new();

//     let test_store = TestStore;
//     engine.adapter().set_store_adapter("store", test_store);
//     let store = store(&engine).await;

//     assert_eq!(store.kind(), StoreKind::Extern);
// }

#[tokio::test]
async fn store_load() {
    let engine = Engine::new();
    let store = store(&engine).await;

    let prefix = utils::shortid();
    for _ in 0..100 {
        let id = format!("{}_{}", prefix, utils::longid());
        let workflow = create_workflow(&id);
        let proc = engine.scher().create_raw_proc(&workflow);
        store.create_proc(&proc);
    }

    let procs = store.load(engine.scher(), 10000);
    let procs = procs
        .iter()
        .filter(|it| it.pid().starts_with(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(procs.len(), 100);
}

#[tokio::test]
async fn store_proc_update() {
    let engine = Engine::new();
    let store = store(&engine).await;

    let id = utils::longid();
    let workflow = create_workflow(&id);
    let proc = engine.scher().create_raw_proc(&workflow);

    store.create_proc(&proc);

    proc.set_state(&TaskState::Running);
    store.update_proc(&proc);

    let p = store.proc(&proc.pid(), &engine.scher()).unwrap();
    assert_eq!(p.pid(), proc.pid());
    assert_eq!(p.state(), TaskState::Running);
}

#[tokio::test]
async fn store_task_update() {
    let engine = Engine::new();
    let store = store(&engine).await;
    engine
        .emitter()
        .on_task(move |task: &Task, data: &EventData| {
            debug!("update task:{}, data={:?}", task.tid(), data);
            if data.action == EventAction::Create {
                store.create_task(task);
            } else {
                store.update_task(task, &data.vars);
            }
        });

    let scher = engine.scher();

    let id = utils::longid();
    let workflow = create_workflow(&id);
    let proc = engine.scher().create_raw_proc(&workflow);
    scher.cache().push(&proc);

    // proc.start();
    scher.sched_proc(&proc);
    // proc.start();
    scher.next().await;
    scher.next().await;
    scher.next().await;
    scher.next().await;

    let tasks = proc.task_by_nid("job1");
    let job = tasks.get(0).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let p = store.proc(&proc.pid(), &engine.scher()).unwrap();

    let tasks = p.task_by_nid("job1");
    let loaded_job = tasks.get(0).unwrap();
    assert_eq!(job.state(), TaskState::Running);
    assert_eq!(loaded_job.state(), TaskState::Running);
}

#[tokio::test]
async fn store_remove() {
    let engine = Engine::new();
    let store = store(&engine).await;

    let id = utils::longid();
    let workflow = create_workflow(&id);
    let proc = engine.scher().create_raw_proc(&workflow);

    store.create_proc(&proc);
    store.remove_proc(&proc.pid());
    let ret = store.proc(&proc.pid(), &engine.scher());
    assert!(ret.is_none());
}

fn create_workflow(id: &str) -> Workflow {
    let text = include_str!("../../../examples/store_test.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    workflow.set_biz_id(id);

    workflow
}

#[derive(Debug)]
struct TestStore;

impl StoreAdapter for TestStore {
    fn procs(&self) -> Arc<dyn DataSet<data::Proc>> {
        todo!()
    }

    fn tasks(&self) -> Arc<dyn DataSet<data::Task>> {
        todo!()
    }

    fn messages(&self) -> Arc<dyn DataSet<data::Message>> {
        todo!()
    }

    fn init(&self) {}

    fn flush(&self) {}
}
