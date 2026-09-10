use crate::{
    Act, Config, Engine, Vars, Workflow,
    config::ConfigData,
    data,
    scheduler::NodeContent,
    scheduler::{NodeTree, Process, Runtime, TaskState},
    store::{DbCollectionIden, MemoryStore},
    utils,
};
use std::sync::Arc;

/// Evicting a finished process must free its whole in-memory graph: the task
/// tree is owned by the process and every task references its process, so a
/// strong `Task.proc` would form a `Process → TaskTree → Task → Process`
/// cycle and the evicted process would leak as unreachable cyclic garbage
/// (`Arc` cannot collect a cycle with no external strong refs). `Task` holds
/// its process `Weak`ly instead — while a caller still holds the process its
/// tasks stay queryable, and once the last holder drops, everything frees.
#[tokio::test]
async fn cache_evict_breaks_proc_task_cycle() {
    let engine = Engine::new().start().await.unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|s| s.with_id("step1"));
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);
    let root = proc
        .create_task(&proc.tree().node("step1").unwrap(), None)
        .unwrap();
    cache.start_proc(&proc, Some(&root)).await.unwrap();
    assert_eq!(cache.count(), 1);
    assert_eq!(proc.tasks().len(), 1);

    let weak = Arc::downgrade(&proc);
    cache.evict(&pid);
    assert_eq!(cache.count(), 0);
    // still held by the caller: the task tree stays queryable
    assert_eq!(proc.tasks().len(), 1);
    // last holder dropped: the process must be deallocated, not kept alive
    // by its own tasks
    drop(proc);
    drop(root);
    assert!(
        weak.upgrade().is_none(),
        "the evicted process must be deallocated — its tasks kept the cycle alive"
    );

    rt.close().await;
}
/// Dynamic act chains must survive a store round-trip: node ids are persisted
/// with parent/prev/next links and rebuilt on load, so `Task::move_next`
/// (which reads `task.node.next()`) keeps working after restore.
#[tokio::test]
async fn cache_restore_dynamic_acts() {
    let engine = Engine::new().start().await.unwrap();
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
        store.upsert_task(&task).await.unwrap();
    }
    store.upsert_proc(&proc).await.unwrap();

    // restore the process from the store
    let restored = store.load_proc(&pid, &rt).await.unwrap().unwrap();
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
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let proc = Process::new(&utils::longid(), &rt);
    cache.push_proc(&proc).await.unwrap();
    assert_eq!(cache.count(), 1);
}

#[tokio::test]
async fn cache_push_get() {
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let pid = utils::longid();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).await.unwrap();
    assert_eq!(cache.count(), 1);

    let proc = cache.proc(&pid, &engine.runtime()).await.unwrap();
    assert!(proc.is_some());
}

#[tokio::test]
async fn cache_push_to_store() {
    let engine = Engine::builder()
        .cache_size(1)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc).await.unwrap();
        pids.push(pid);
    }

    // the resident set has no eviction policy — `cache_cap` gates *starts*
    // (`Cache::admit` parks over-cap ones), it never evicts resident
    // processes, so all five pushed processes stay in memory
    assert_eq!(cache.count(), 5);
    for pid in pids.iter() {
        let exists = cache.store().procs().exists(pid).await.unwrap();
        assert!(exists);
    }
}

#[tokio::test]
async fn cache_remove() {
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();

    let mut pids = Vec::new();
    for _ in 0..5 {
        let pid = utils::longid();
        let proc = Process::new(&pid, &rt);
        cache.push_proc(&proc).await.unwrap();
        pids.push(pid);
    }

    assert_eq!(cache.count(), 5);
    for pid in pids.iter() {
        let exists = cache.store().procs().exists(pid).await.unwrap();
        assert!(exists);

        cache.remove(pid).await.unwrap();
        assert!(cache.proc(pid, &engine.runtime()).await.unwrap().is_none());

        let exists = cache.store().procs().exists(pid).await.unwrap();
        assert!(!exists);
    }
    assert_eq!(cache.count(), 0);
}

#[tokio::test]
async fn cache_upsert() {
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));

    let pid = utils::longid();
    let tree = NodeTree::build(&mut workflow).unwrap();

    let cache = rt.cache();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).await.unwrap();
    assert_eq!(cache.count(), 1);

    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None).unwrap();

    proc.set_state(TaskState::Running);
    cache.upsert(&task).await.unwrap();

    let proc = cache.proc(&pid, &engine.runtime()).await.unwrap().unwrap();
    assert_eq!(proc.state(), TaskState::Running);
}

/// `Cache::remove` is serialized through the store writer (FIFO): a task
/// write queued before the removal is applied first, and then every row of
/// the process (proc, tasks, outbox ops) is dropped — one flush reports no
/// failure, so removal can never race the writes still queued behind it.
#[tokio::test]
async fn cache_remove_after_writer_writes_drops_all_rows() {
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let store = cache.store();

    let pid = utils::longid();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).await.unwrap();
    assert!(store.procs().exists(&pid).await.unwrap());

    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None).unwrap();
    let tid = task.id.clone();
    // the persisted row id is the composite pid-tid
    let task_row_id = utils::Id::new(&pid, &tid).id();

    // queue a task write on the writer and remove without flushing first:
    // remove() must drain the queue (the task write applies) before it drops
    // the rows
    proc.set_state(TaskState::Completed);
    cache.upsert_async(&task).unwrap();
    cache.remove(&pid).await.unwrap();

    assert!(!store.procs().exists(&pid).await.unwrap());
    assert!(store.tasks().find(&task_row_id).await.is_err());
    assert!(cache.proc(&pid, &rt).await.unwrap().is_none());
    cache.flush().await.unwrap();
}

/// A task write that reaches the writer after its process was removed is
/// dead data: it is skipped — neither applied (which would resurrect the
/// rows) nor failed (which would poison a later flush).
#[tokio::test]
async fn cache_writes_after_remove_are_skipped() {
    let engine = Engine::builder()
        .cache_size(10)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let store = cache.store();

    let pid = utils::longid();
    let proc = Process::new(&pid, &rt);
    cache.push_proc(&proc).await.unwrap();

    let mut workflow = Workflow::new().with_step(|step| step.with_name("step1"));
    let tree = NodeTree::build(&mut workflow).unwrap();
    let node = tree.root.as_ref().unwrap();
    let task = proc.create_task(node, None).unwrap();
    let tid = task.id.clone();
    // the persisted row id is the composite pid-tid
    let task_row_id = utils::Id::new(&pid, &tid).id();

    proc.set_state(TaskState::Completed);
    cache.upsert_async(&task).unwrap();
    cache.flush().await.unwrap();
    assert!(store.tasks().find(&task_row_id).await.is_ok());

    cache.remove(&pid).await.unwrap();
    assert!(store.tasks().find(&task_row_id).await.is_err());

    // late write for the removed process: skipped silently
    cache.upsert_async(&task).unwrap();
    cache.flush().await.unwrap();
    assert!(
        store.tasks().find(&task_row_id).await.is_err(),
        "late write resurrected the task row of a removed process"
    );
    assert!(!store.procs().exists(&pid).await.unwrap());
}

/// `start_parked` touches ONLY parked rows (durable `None` state): it starts them
/// into free slots and leaves every other row alone. Non-`None` non-terminal
/// rows (`Ready`/`Running`/`Pending`) belong to processes with no in-memory
/// executor to drive them — a live process's row is its resident instance
/// (reloading it would create a second one), and a crash-left one is reached
/// on demand (`proc()`) — so `start_parked` must NOT pull them into the cache, and
/// terminal rows are never refilled either. Runtime built WITHOUT an event
/// loop so the started seeds stay `Running` and every assertion is
/// deterministic.
#[tokio::test]
async fn cache_start_parked_refills_only_parked_none_rows() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(5),
            ..Default::default()
        },
        table: Default::default(),
    };
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    cache.store().deploy(&model, None).await.unwrap();

    let seed = |state: TaskState| data::Proc {
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
        removable: false,
        v: data::Proc::version(),
    };

    assert_eq!(cache.count(), 0);

    // parked (None) rows that restore MUST start, oldest first
    let parked = [
        seed(TaskState::None),
        seed(TaskState::None),
        seed(TaskState::None),
    ];
    let parked_ids: Vec<String> = parked.iter().map(|p| p.id.clone()).collect();
    // rows restore MUST leave alone: crash-left working states + finished
    let ignored = [
        seed(TaskState::Ready),
        seed(TaskState::Running),
        seed(TaskState::Pending),
        seed(TaskState::Completed),
        seed(TaskState::Error),
    ];
    for proc in parked.into_iter().chain(ignored.into_iter()) {
        cache.store().procs().create(&proc).await.unwrap();
    }

    cache.start_parked(&rt).await.unwrap();

    // exactly the three parked rows were started — the non-None/terminal
    // seeds stay out of the resident set
    assert_eq!(cache.count(), 3);
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    for pid in &parked_ids {
        assert!(resident.contains(pid), "parked row {pid} must be started");
        let row = cache.store().procs().find(pid).await.unwrap();
        assert!(
            TaskState::from(row.state.as_str()).is_running(),
            "parked row {pid} must be Running"
        );
    }
    let unexpected: Vec<&String> = resident
        .iter()
        .filter(|p| !parked_ids.contains(p))
        .collect();
    assert!(
        unexpected.is_empty(),
        "start_parked must not load non-parked rows: {unexpected:?}"
    );

    rt.close().await;
}

/// A finished process is evicted from the in-memory cache on its terminal
/// proc event (its store rows stay — the sweeper deletes them only after the
/// process's deliveries settled), so the freed slot lets the restore pass
/// start parked processes (`None` state) into it. Without the eviction,
/// finished processes would squat in the cache and block restoring others.
#[tokio::test(flavor = "multi_thread")]
async fn cache_finished_proc_frees_slot_for_restore() {
    let engine = Engine::builder()
        .cache_size(4)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let store = cache.store();

    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    store.deploy(&model, None).await.unwrap();

    // three processes persisted but never started (a crash left them behind)
    let mut seeds = Vec::new();
    for _ in 0..3 {
        let pid = utils::longid();
        let proc = data::Proc {
            id: pid.clone(),
            name: "seed".to_string(),
            mid: "m1".to_string(),
            state: TaskState::None.into(),
            start_time: 0,
            end_time: 0,
            timestamp: 0,
            model: model.to_json().unwrap(),
            env: "{}".to_string(),
            err: None,
            removable: false,
            v: data::Proc::version(),
        };
        store.procs().create(&proc).await.unwrap();
        seeds.push(pid);
    }

    // two real processes run concurrently; while both are resident the cache
    // count (2) sits at the restore checkpoint of cap 4 (cap/2 = 2), so no
    // restore pass starts — only their terminal eviction drops the count
    // below the checkpoint and lets the seeds be restored
    let (a, b) = tokio::join!(rt.start(&model, Vars::new()), rt.start(&model, Vars::new()));
    let running = [a.unwrap(), b.unwrap()];
    let mut pids = seeds.clone();
    pids.extend(running.iter().map(|p| p.id().to_string()));

    // every process — the two that ran and the three restored seeds — must
    // end up terminal in the store (or gone, swept after its rows settled);
    // under the old behavior the finished procs stayed cached and the seeds
    // were never restored, so they would remain `None` forever
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let mut done = true;
            for pid in &pids {
                if let Ok(row) = store.procs().find(pid).await
                    && !TaskState::from(row.state.as_str()).is_completed()
                {
                    done = false;
                }
            }
            if done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("finished/restored processes never reached a terminal state in time");
}

/// The resident set has no eviction policy: once it is full a new process is
/// *parked* — its durable row stays `None` and it is NOT cached (a resident
/// parked row would occupy a slot forever, since `restore` skips resident
/// pids) — instead of evicting a live process to make room. A terminal event
/// then frees a slot and `restore` starts the parked process, which runs to
/// a terminal row of its own.
#[tokio::test]
async fn cache_park_over_cap_then_refill_on_terminal() {
    let engine = Engine::builder()
        .cache_size(2)
        .build()
        .start()
        .await
        .unwrap();
    let rt = engine.runtime();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    cache.store().deploy(&model, None).await.unwrap();

    let make = |tag: &str| {
        let pid = format!("park-{tag}");
        let proc = Process::new(&pid, &rt);
        proc.load(&model).unwrap();
        (pid, proc)
    };

    let (pid1, p1) = make("1");
    let (pid2, p2) = make("2");
    let (pid3, p3) = make("3");

    // two admitted starts fill the resident set exactly to cap
    assert!(cache.admit(&p1).await.unwrap());
    assert!(cache.admit(&p2).await.unwrap());
    assert_eq!(cache.count(), 2);

    // the third is parked: durable `None` row, no resident slot taken, and
    // the admitted processes are never evicted to make room for it
    assert!(!cache.admit(&p3).await.unwrap());
    assert_eq!(cache.count(), 2);
    let row = cache.store().procs().find(&pid3).await.unwrap();
    assert_eq!(row.state, TaskState::None.to_string());
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    assert!(resident.contains(&pid1));
    assert!(resident.contains(&pid2));
    assert!(!resident.contains(&pid3));

    // a demand load of a parked process must not cache it
    let got = cache.proc(&pid3, &rt).await.unwrap().unwrap();
    assert_eq!(got.id(), pid3);
    assert!(got.state().is_none());
    assert_eq!(cache.count(), 2);

    // terminal event: pid1 frees its slot; restore refills it with the
    // parked process and starts it
    cache.evict(&pid1);
    cache.start_parked(&rt).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match cache.store().procs().find(&pid3).await {
                Ok(row) if !TaskState::from(row.state.as_str()).is_completed() => {}
                // completed, or already swept away after its rows settled
                _ => break,
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("parked process never ran to completion after refill");
}

/// Concurrent cache misses for the same pid must coalesce into one store load
/// and one in-memory instance. Returning independently loaded `Arc<Process>`
/// values would let callers drive the same durable process with two state trees.
#[tokio::test(flavor = "multi_thread")]
async fn cache_concurrent_proc_miss_returns_one_instance() {
    let config = Config::default();
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    cache.store().deploy(&model, None).await.unwrap();

    let pid = utils::longid();
    let row = data::Proc {
        id: pid.clone(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: TaskState::Running.to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: model.to_json().unwrap(),
        env: "{}".to_string(),
        err: None,
        removable: false,
        v: data::Proc::version(),
    };
    cache.store().procs().create(&row).await.unwrap();

    let mut handles = Vec::new();
    for _ in 0..50 {
        let cache = cache.clone();
        let rt = rt.clone();
        let pid = pid.clone();
        handles.push(tokio::spawn(async move {
            cache.proc(&pid, &rt).await.unwrap().unwrap()
        }));
    }
    let mut handles = handles.into_iter();
    let first = handles.next().unwrap().await.unwrap();
    let expected = Arc::as_ptr(&first);
    let mut pointers = std::collections::HashSet::from([expected]);
    for handle in handles {
        let proc = handle.await.unwrap();
        assert!(Arc::ptr_eq(&first, &proc));
        pointers.insert(Arc::as_ptr(&proc));
    }
    assert_eq!(pointers.len(), 1);
    assert_eq!(cache.count(), 1);

    rt.close().await;
}

/// Admission claims a pid atomically. Concurrent fresh starts that all missed
/// the durable row can therefore produce only one executable instance.
#[tokio::test(flavor = "multi_thread")]
async fn cache_admit_same_pid_has_single_winner() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(1),
            ..Default::default()
        },
        table: Default::default(),
    };
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));

    let pid = "concurrent-admit";
    let make = || {
        let proc = Process::new(pid, &rt);
        proc.load(&model).unwrap();
        proc
    };

    let mut handles = Vec::new();
    for _ in 0..20 {
        let cache = cache.clone();
        let proc = make();
        handles.push(tokio::spawn(async move { cache.admit(&proc).await }));
    }
    let mut admitted = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(true) => admitted += 1,
            Ok(false) => panic!("cap is 1, so no second process should be parked"),
            Err(err) => assert!(err.to_string().contains("duplicated")),
        }
    }
    assert_eq!(admitted, 1);
    assert_eq!(cache.count(), 1);

    rt.close().await;
}

/// Parked rows refill FIFO (oldest first) and a refill never overshoots
/// `cap`: freeing one slot starts exactly the oldest parked process; newer
/// parked rows keep waiting for the next terminal event. The runtime is
/// built WITHOUT an event loop, so started processes stay `Running` (their
/// tasks sit in the queue) and every assertion is deterministic.
#[tokio::test]
async fn cache_parked_refill_is_oldest_first_within_cap() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(2),
            ..Default::default()
        },
        table: Default::default(),
    };
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_name("step1"));
    cache.store().deploy(&model, None).await.unwrap();

    let make = |tag: &str, ts: i64| {
        let pid = format!("park-fifo-{tag}");
        let proc = Process::new_with_timestamp(&pid, ts, &rt);
        proc.load(&model).unwrap();
        (pid, proc)
    };

    let (pid1, p1) = make("1", 100);
    let (_pid2, p2) = make("2", 200);
    let (old, p_old) = make("old", 10);
    let (new, p_new) = make("new", 999);

    assert!(cache.admit(&p1).await.unwrap());
    assert!(cache.admit(&p2).await.unwrap());
    assert_eq!(cache.count(), 2);

    // two parked rows, older first
    assert!(!cache.admit(&p_old).await.unwrap());
    assert!(!cache.admit(&p_new).await.unwrap());

    // freeing one slot starts only the oldest parked process — the newer one
    // keeps waiting, so the resident set never exceeds cap
    cache.evict(&pid1);
    cache.start_parked(&rt).await.unwrap();
    assert_eq!(cache.count(), 2);
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    assert!(
        resident.contains(&old),
        "oldest parked row must be refilled first: {resident:?}"
    );
    assert!(!resident.contains(&new));
    let still = cache.store().procs().find(&new).await.unwrap();
    assert_eq!(still.state, TaskState::None.to_string());
    let started = cache.store().procs().find(&old).await.unwrap();
    assert!(TaskState::from(started.state.as_str()).is_running());

    rt.close().await;
}

/// Scope vars live in their own rows: a lifecycle-only persist (state/timing
/// change, no data touched) must NOT write a vars row, and a data mutation
/// must create one with exactly that scope's content.
#[tokio::test]
async fn cache_vars_row_written_only_on_mutation() {
    let engine = Engine::new().start().await.unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();

    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|s| s.with_id("step1"));
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);
    let root = proc
        .create_task(&proc.tree().node("step1").unwrap(), None)
        .unwrap();
    let task_id = utils::Id::new(&pid, &root.id).id();
    proc.set_state(TaskState::Running);

    // 1. lifecycle-only write: state/time changed, vars untouched — no vars
    // row is created and the lifecycle row carries no scope vars
    root.set_pure_state(TaskState::Running);
    root.set_start_time(1);
    store.persist_task_rows(&root).await.unwrap();
    assert!(
        store.vars().find(&task_id).await.is_err(),
        "a lifecycle-only write must not write a scope vars row"
    );
    let row = store.tasks().find(&task_id).await.unwrap();
    let json = serde_json::to_string(&row).unwrap();
    assert!(
        !json.contains("\"data\"") && !json.contains("\"sealed\""),
        "the lifecycle row must not carry scope vars: {json}"
    );

    // 2. a data mutation flushes exactly the mutated scope's vars row
    root.set_data(&Vars::new().with("var1", 10));
    store.persist_task_rows(&root).await.unwrap();
    let vars = store.vars().find(&task_id).await.unwrap();
    let data: Vars = serde_json::from_str(&vars.data).unwrap();
    assert_eq!(data.get::<i32>("var1").unwrap(), 10);
    assert!(
        !root.is_vars_dirty(),
        "vars dirty flag must clear after the flush"
    );

    // 3. a later lifecycle-only write does not rewrite the vars row
    root.set_pure_state(TaskState::Completed);
    root.set_end_time(2);
    store.persist_task_rows(&root).await.unwrap();
    let vars = store.vars().find(&task_id).await.unwrap();
    let data: Vars = serde_json::from_str(&vars.data).unwrap();
    assert_eq!(
        data.get::<i32>("var1").unwrap(),
        10,
        "vars row must not be rewritten"
    );
}

/// A data write that lands in an ancestor scope — the classic "child output
/// folds up to the declaring owner" case — persists exactly that owner's
/// vars row, and restore re-attaches it: the store round-trip keeps the
/// ancestor's updated vars without ever touching the root row.
#[tokio::test]
async fn cache_vars_ancestor_scope_round_trip() {
    let engine = Engine::new().start().await.unwrap();
    let rt = engine.runtime();
    let store = rt.cache().store();

    let workflow = Workflow::new()
        .with_id("m1")
        .with_step(|s| s.with_id("step1"));
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &workflow);
    proc.set_state(TaskState::Running);

    // root (workflow) > step1 > act; the act's output folds up to step1,
    // the scope that declares it
    let root_node = proc.tree().root.clone().unwrap();
    let root = proc.create_task(&root_node, None).unwrap();
    let step1_node = proc.tree().node("step1").unwrap();
    let step1 = proc.create_task(&step1_node, Some(root.clone())).unwrap();
    let act_id = utils::shortid();
    {
        let tree = proc.tree();
        let act = Act::irq(|r| r.with_params_vars(|v| v.with("key", "a1"))).with_id(&act_id);
        let node = tree
            .append_node(
                &step1_node,
                &act_id,
                NodeContent::Act(act),
                step1_node.level + 1,
            )
            .unwrap();
        node.set_parent(&step1_node);
    }
    let act_node = proc.tree().node(&act_id).unwrap();
    let act = proc.create_task(&act_node, Some(step1.clone())).unwrap();
    let step1_tid = step1.id.clone();
    let root_tid = root.id.clone();

    // step1 declares `x`; the act writes it → step1's scope owns the value
    step1.set_data_with(|data| data.set("x", 1));
    store.persist_task_rows(&step1).await.unwrap();
    store.persist_task_rows(&root).await.unwrap();
    assert!(
        store
            .vars()
            .find(&utils::Id::new(&pid, &root_tid).id())
            .await
            .is_err(),
        "root scope has no vars row — it never mutated"
    );

    act.set_data_with(|data| data.set("x", 2));
    act.update_data(&act.data());
    assert!(
        step1.is_vars_dirty(),
        "the owner scope must be marked dirty"
    );
    assert!(!root.is_vars_dirty(), "the root scope must stay untouched");

    store.persist_task_rows(&act).await.unwrap();
    let step1_vars = store
        .vars()
        .find(&utils::Id::new(&pid, &step1_tid).id())
        .await
        .unwrap();
    let step1_data: Vars = serde_json::from_str(&step1_vars.data).unwrap();
    assert_eq!(
        step1_data.get::<i32>("x").unwrap(),
        2,
        "owner scope row updated"
    );
    assert!(
        store
            .vars()
            .find(&utils::Id::new(&pid, &root_tid).id())
            .await
            .is_err(),
        "root scope still has no vars row"
    );

    // restore: the step scope's vars re-attach to the reloaded task
    store.upsert_proc(&proc).await.unwrap();
    let restored = store.load_proc(&pid, &rt).await.unwrap().unwrap();
    let step1 = restored.task(&step1_tid).unwrap();
    assert_eq!(
        step1.with_data(|d| d.get::<i32>("x")),
        Some(2),
        "restored owner scope keeps its updated var"
    );
    assert_eq!(
        restored
            .task(&root_tid)
            .unwrap()
            .with_data(|d| d.get::<i32>("x")),
        None,
        "the untouched root scope stays empty"
    );
}

/// Boot-time resume priority: in-flight (`Ready`/`Running`/`Pending`) rows
/// are loaded into the resident set before parked (`None`) rows, both capped;
/// everything beyond the cap waits. Deterministic — the runtime has no event
/// loop, so nothing executes.
#[tokio::test]
async fn cache_resume_loads_in_flight_before_parked() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(3),
            ..Default::default()
        },
        table: Default::default(),
    };
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));
    cache.store().deploy(&model, None).await.unwrap();

    // an in-flight process with a task graph (a crash mid-run)
    let inflight = {
        let proc = Process::new_with_timestamp("resume-inflight", 5, &rt);
        proc.load(&model).unwrap();
        proc.set_pure_state(TaskState::Running);
        proc
    };
    let root = inflight
        .create_task(&inflight.tree().root.clone().unwrap(), None)
        .unwrap();
    let step1 = inflight
        .create_task(&inflight.tree().node("step1").unwrap(), Some(root.clone()))
        .unwrap();
    root.set_pure_state(TaskState::Running);
    step1.set_pure_state(TaskState::Ready);
    cache.store().upsert_proc(&inflight).await.unwrap();
    cache.store().upsert_task(&root).await.unwrap();
    cache.store().upsert_task(&step1).await.unwrap();

    // three parked rows (never started): two fit the free slots left by the
    // in-flight process under cap 3, the third must keep waiting
    let mut parked = Vec::new();
    for (tag, ts) in [("a", 10), ("b", 20), ("new", 999)] {
        let pid = format!("resume-parked-{tag}");
        let proc = Process::new_with_timestamp(&pid, ts, &rt);
        proc.load(&model).unwrap();
        proc.set_pure_state(TaskState::None);
        cache.store().upsert_proc(&proc).await.unwrap();
        parked.push(pid);
    }

    assert_eq!(cache.count(), 0);
    rt.resume().await.unwrap();

    // cap 3: the in-flight process is resumed first, then the two oldest
    // parked rows are started into the free slots; the newest keeps waiting
    assert_eq!(cache.count(), 3);
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    assert!(
        resident.contains(&"resume-inflight".to_string()),
        "in-flight process must be loaded first: {resident:?}"
    );
    assert!(resident.contains(&"resume-parked-a".to_string()));
    assert!(resident.contains(&"resume-parked-b".to_string()));
    assert!(!resident.contains(&"resume-parked-new".to_string()));
    let waiting = cache
        .store()
        .procs()
        .find("resume-parked-new")
        .await
        .unwrap();
    assert_eq!(waiting.state, TaskState::None.to_string());

    // the in-flight process's task graph was decoded and re-dispatched
    let loaded = cache.proc("resume-inflight", &rt).await.unwrap().unwrap();
    assert!(loaded.state().is_running());
    assert_eq!(
        loaded.task_by_nid("step1").first().unwrap().state(),
        TaskState::Ready
    );

    rt.close().await;
}

/// A process that was mid-run when the engine died is resumed by a fresh
/// engine on the same store: its in-flight tasks are re-dispatched and the
/// workflow runs to its terminal state instead of hanging forever.
#[tokio::test(flavor = "multi_thread")]
async fn cache_resume_in_flight_proc_after_restart() {
    let kv: Arc<dyn crate::store::KvStore> = Arc::new(MemoryStore::new());

    // engine 1 seeds the store with a mid-run process, then "crashes"
    let engine1 = Engine::builder()
        .set_store(kv.clone())
        .build()
        .start()
        .await
        .unwrap();
    let store = engine1.runtime().cache().store();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));
    store.deploy(&model, None).await.unwrap();

    let pid = "resume-restart".to_string();
    let proc = engine1.runtime().create_proc(&pid, &model);
    proc.set_pure_state(TaskState::Running);
    let root = proc
        .create_task(&proc.tree().root.clone().unwrap(), None)
        .unwrap();
    let step1 = proc
        .create_task(&proc.tree().node("step1").unwrap(), Some(root.clone()))
        .unwrap();
    root.set_pure_state(TaskState::Running);
    step1.set_pure_state(TaskState::Ready);
    store.upsert_proc(&proc).await.unwrap();
    store.upsert_task(&root).await.unwrap();
    store.upsert_task(&step1).await.unwrap();
    engine1.close().await;

    // engine 2 on the same store resumes the process to completion
    let engine2 = Engine::builder()
        .set_store(kv.clone())
        .build()
        .start()
        .await
        .unwrap();
    let store2 = engine2.runtime().cache().store();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match store2.procs().find(&pid).await {
                Ok(row) if !TaskState::from(row.state.as_str()).is_completed() => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                // completed, or swept away after its rows settled
                _ => break,
            }
        }
    })
    .await
    .expect("resumed process never reached a terminal state after restart");
    engine2.close().await;
}

/// Boot-resume overflow is not stranded: in-flight rows beyond the resident
/// cap are queued, and every slot freed by a terminal event (`refill`) loads
/// one of them back — `restore` alone could never, since it only refills
/// parked (`None`) rows. Deterministic — no event loop, so nothing executes.
#[tokio::test]
async fn cache_resume_overflow_drains_on_free_slot() {
    let config = Config {
        data: ConfigData {
            cache_cap: Some(2),
            ..Default::default()
        },
        table: Default::default(),
    };
    let rt = Runtime::new(&config, None).unwrap();
    let cache = rt.cache();
    let model = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("step1"));
    cache.store().deploy(&model, None).await.unwrap();

    // four in-flight rows, only two fit
    let mut pids = Vec::new();
    for i in 0..4 {
        let pid = format!("overflow-{i}");
        let proc = Process::new_with_timestamp(&pid, i as i64 + 1, &rt);
        proc.load(&model).unwrap();
        proc.set_pure_state(TaskState::Running);
        cache.store().upsert_proc(&proc).await.unwrap();
        pids.push(pid);
    }

    assert_eq!(cache.count(), 0);
    rt.resume().await.unwrap();
    assert_eq!(cache.count(), 2);
    // the two overflow rows are queued for the next free slots
    assert_eq!(
        cache.pending_resume_ids(),
        vec![pids[2].clone(), pids[3].clone()]
    );

    // a terminal event frees pid0's slot: the oldest queued process is loaded
    cache.evict(&pids[0]);
    rt.restore().await.unwrap();
    assert_eq!(cache.count(), 2);
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    assert!(resident.contains(&pids[1]));
    assert!(resident.contains(&pids[2]));
    assert_eq!(cache.pending_resume_ids(), vec![pids[3].clone()]);

    // another terminal event frees pid1's slot: the last queued row is loaded
    cache.evict(&pids[1]);
    rt.restore().await.unwrap();
    assert_eq!(cache.count(), 2);
    let resident: Vec<String> = cache.procs().iter().map(|p| p.id().to_string()).collect();
    assert!(resident.contains(&pids[2]));
    assert!(resident.contains(&pids[3]));
    assert!(cache.pending_resume_ids().is_empty());

    rt.close().await;
}
