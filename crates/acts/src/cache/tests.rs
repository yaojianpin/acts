use crate::{
    Act, Engine, Workflow, data,
    scheduler::NodeContent,
    scheduler::{NodeTree, Process, TaskState},
    store::DbCollectionIden,
    utils,
};
/// Dynamic act chains must survive a store round-trip: node ids are persisted
/// with parent/prev/next links and rebuilt on load, so `Task::move_next`
/// (which reads `task.node.next()`) keeps working after restore.
#[tokio::test]
async fn cache_restore_dynamic_acts() {
    let engine = Engine::new().start().unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();

    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);

    // build the dynamic act chain like ctx.build_acts does
    let act_ids;
    {
        let tree = proc.tree();
        let step1 = tree.node("step1").unwrap();
        let mut prev = step1.clone();
        let mut acts = [
            Act::irq(|r| r.with_params_vars(|v| v.with("key", "act1"))),
            Act::irq(|r| r.with_params_vars(|v| v.with("key", "act2"))),
            Act::irq(|r| r.with_params_vars(|v| v.with("key", "act3"))),
        ];
        for act in acts.iter_mut() {
            if act.id.is_empty() {
                act.id = utils::shortid();
            }
            let node = tree
                .append_node(
                    &step1,
                    &act.id,
                    NodeContent::Act(act.clone()),
                    step1.level + 1,
                )
                .unwrap();
            if node.level == prev.level {
                prev.set_next(&node, true);
            } else {
                node.set_parent(&step1);
            }
            prev = node;
        }
        act_ids = acts.iter().map(|a| a.id.clone()).collect::<Vec<_>>();
    }

    // create tasks for the chain like sched_task does, then save everything
    let step_node = proc.tree().node("step1").unwrap();
    let step_task = proc.create_task(&step_node, None).unwrap();
    let mut prev_task = step_task;
    for id in &act_ids {
        let node = proc.tree().node(id).unwrap();
        let task = proc.create_task(&node, Some(prev_task.clone())).unwrap();
        prev_task = task;
    }
    for task in proc.tasks() {
        store.upsert_task(&task).unwrap();
    }
    store.upsert_proc(&proc).unwrap();

    // restore the process from the store
    let restored = store.load_proc(&pid, &rt).unwrap().unwrap();
    let act_tasks = restored
        .tasks()
        .into_iter()
        .filter(|t| t.node().kind() == crate::scheduler::NodeKind::Act)
        .collect::<Vec<_>>();
    assert_eq!(act_tasks.len(), 3);

    let a1 = restored.tree().node(&act_ids[0]).unwrap();
    let a2 = restored.tree().node(&act_ids[1]).unwrap();
    let a3 = restored.tree().node(&act_ids[2]).unwrap();
    // node chain restored: what Task::move_next reads
    assert_eq!(a1.next().upgrade().unwrap().id(), a2.id());
    assert_eq!(a2.prev().upgrade().unwrap().id(), a1.id());
    assert_eq!(a2.next().upgrade().unwrap().id(), a3.id());
    assert!(a3.next().upgrade().is_none());
    // parent and children restored
    assert_eq!(a1.parent().unwrap().id(), "step1");
    let step = restored.tree().node("step1").unwrap();
    assert_eq!(step.children().len(), 1);
    assert_eq!(step.children()[0].id(), a1.id());
}

#[tokio::test]
async fn cache_count() {
    let engine = Engine::builder().cache_size(10).build().start().unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let proc = Process::new(&utils::longid(), &rt);
    cache.push_proc(&proc).unwrap();
    assert_eq!(cache.count(), 1);
}

#[tokio::test]
async fn cache_push_get() {
    let engine = Engine::builder().cache_size(10).build().start().unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let pid = utils::longid();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).unwrap();
    assert_eq!(cache.count(), 1);

    let proc = cache.proc(&pid, &engine.runtime()).unwrap();
    assert!(proc.is_some());
}

#[tokio::test]
async fn cache_push_to_store() {
    let engine = Engine::builder().cache_size(1).build().start().unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc).unwrap();
        pids.push(pid);
    }

    assert_eq!(cache.count(), 1);
    for pid in pids.iter() {
        let exists = cache.store().procs().exists(pid).unwrap();
        assert!(exists);
    }
}

#[tokio::test]
async fn cache_remove() {
    let engine = Engine::builder().cache_size(10).build().start().unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc).unwrap();
        pids.push(pid);
    }

    assert_eq!(cache.count(), 5);
    for pid in pids.iter() {
        let exists = cache.store().procs().exists(pid).unwrap();
        assert!(exists);

        cache.remove(pid).unwrap();
        assert!(cache.proc(pid, &engine.runtime()).unwrap().is_none());

        let exists = cache.store().procs().exists(pid).unwrap();
        assert!(!exists);
    }
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn cache_upsert() {
    let engine = Engine::builder().cache_size(10).build().start().unwrap();
    let rt = engine.runtime();
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tree = NodeTree::build(&mut workflow).unwrap();

    let cache = rt.cache();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).unwrap();
    assert_eq!(cache.count(), 1);

    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None).unwrap();

    proc.set_state(TaskState::Running);
    cache.upsert(&task).unwrap();

    let proc = cache.proc(&pid, &engine.runtime()).unwrap().unwrap();
    assert_eq!(proc.state(), TaskState::Running);
}

#[tokio::test]
async fn cache_restore_count() {
    let engine = Engine::builder().cache_size(5).build().start().unwrap();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let rt = engine.runtime();
    let cache = rt.cache();
    cache.store().deploy(&model, None).unwrap();

    assert_eq!(cache.count(), 0);
    for _ in 0..10 {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env: "{}".to_string(),
            err: None,
            v: data::Proc::version(),
        };
        cache.store().procs().create(&proc).unwrap();
    }

    cache.restore(&engine.runtime()).unwrap();
    assert_eq!(cache.count(), 5);
}

#[tokio::test]
async fn cache_restore_working_state() {
    let engine = Engine::builder().cache_size(5).build().start().unwrap();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let rt = engine.runtime();
    let cache = rt.cache();
    cache.store().deploy(&model, None).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [
        TaskState::None,
        TaskState::None,
        TaskState::None,
        TaskState::Running,
        TaskState::Running,
        TaskState::Running,
        TaskState::Pending,
        TaskState::Pending,
        TaskState::Pending,
        TaskState::Pending,
    ];
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env: "{}".to_string(),
            err: None,
            v: data::Proc::version(),
        };
        cache.store().procs().create(&proc).unwrap();
    }

    cache.restore(&engine.runtime()).unwrap();
    assert_eq!(cache.count(), 5);
}

#[tokio::test]
async fn cache_restore_completed_state() {
    let engine = Engine::builder().cache_size(5).build().start().unwrap();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let rt = engine.runtime();
    let cache = rt.cache();
    cache.store().deploy(&model, None).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [
        TaskState::Skipped,
        TaskState::Skipped,
        TaskState::Skipped,
        TaskState::Aborted,
        TaskState::Aborted,
        TaskState::Aborted,
        TaskState::Error,
        TaskState::Error,
        TaskState::Completed,
        TaskState::Completed,
    ];
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env: "{}".to_string(),
            err: None,
            v: data::Proc::version(),
        };
        cache.store().procs().create(&proc).unwrap();
    }

    cache.restore(&engine.runtime()).unwrap();
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn cache_restore_less_cap() {
    let engine = Engine::builder().cache_size(5).build().start().unwrap();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    let rt = engine.runtime();
    let cache = rt.cache();
    cache.store().deploy(&model, None).unwrap();

    assert_eq!(cache.count(), 0);

    let states = [TaskState::Running, TaskState::None, TaskState::Pending];
    for state in &states {
        let proc = data::Proc {
            id: utils::longid(),
            name: "test".to_string(),
            mid: "m1".to_string(),
            state: state.to_string(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env: "{}".to_string(),
            err: None,
            v: data::Proc::version(),
        };
        cache.store().procs().create(&proc).unwrap();
    }

    cache.restore(&engine.runtime()).unwrap();
    assert_eq!(cache.count(), 3);
}
