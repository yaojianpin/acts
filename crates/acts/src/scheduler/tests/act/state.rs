use crate::{
    MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    utils::{
        self,
        test::{USES_IRQ, auto_complete, create_proc},
    },
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_submitted() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt2.do_action2(&e.pid, &e.tid, EventAction::Submit, Vars::new())
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Submitted) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_completed() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Completed) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_skipped() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt2.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Skipped) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_backed() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                    .await
                    .unwrap();
            }
            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                let mut options = Vars::new();
                options.set("to", "step1");
                rt2.do_action2(&e.pid, &e.tid, EventAction::Back, options)
                    .await
                    .unwrap();
            }
            if e.is_params_key("act2") && e.is_state(MessageState::Backed) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_cancelled() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();

    let act1_tid = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let act1_tid_clone = act1_tid.clone();

    engine.channel().on_message(move |e| {
        let act1_tid_clone = act1_tid_clone.clone();
        let act1_tid = act1_tid.clone();
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                *act1_tid_clone.lock() = e.tid.clone();
                rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                    .await
                    .unwrap();
            }

            if e.is_params_key("act2") && e.is_state(MessageState::Created) {
                let tid = act1_tid.lock().clone();
                let mut options = Vars::new();
                options.set("to", "step1");
                rt2.do_action2(&e.pid, &tid, EventAction::Cancel, options)
                    .await
                    .unwrap();
            }

            if e.is_params_key("act2") && e.is_state(MessageState::Cancelled) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_error() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                let mut options = Vars::new();
                options.set("ecode", "err1");
                rt2.do_action2(&e.pid, &e.tid, EventAction::Error, options)
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Error) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_interrupt() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    let proc2 = proc.clone();
    engine.channel().on_message(move |e| {
        let proc2 = proc2.clone();
        let tx2 = tx2.clone();
        let rt2 = rt2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                // IRQ act task should be in Interrupt state
                let tasks = proc2.task_by_nid(&e.nid);
                if let Some(task) = tasks.first()
                    && task.state() == TaskState::Interrupt
                {
                    tx2.send(true);
                }
                // Complete the act so workflow can finish
                rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                    .await
                    .unwrap();
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_aborted() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                let mut options = Vars::new();
                options.set("uid", "u1");
                rt2.do_action2(&e.pid, &e.tid, EventAction::Abort, options)
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Aborted) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_removed() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    let tx2 = tx.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        let rt2 = rt2.clone();
        let tx2 = tx2.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt2.do_action2(&e.pid, &e.tid, EventAction::Remove, Vars::new())
                    .await
                    .unwrap();
            }
            if e.is_params_key("act1") && e.is_state(MessageState::Removed) {
                tx2.send(true);
            }
        }
    });
    rt.launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_act_state_set_process_var() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(false).double();
    auto_complete(&engine, &rx);
    engine.channel().on_message(move |e| {
        let rt = rt.clone();
        let rx = rx.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rt.do_action2(
                    &e.pid,
                    &e.tid,
                    EventAction::SetProcessVars,
                    Vars::new().with("var1", 1),
                )
                .await
                .unwrap();
                rx.send(true);
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    assert!(ret);
    assert_eq!(
        proc.task_by_params("key", "act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .vars()
            .get::<i32>("var1")
            .unwrap(),
        1
    );
    assert_eq!(
        proc.task_by_nid("step1").first().unwrap().state(),
        TaskState::Running
    );
    proc.print();
    engine.runtime().cache().flush().await.unwrap();

    // test the new var is stored
    let id = utils::Id::new(proc.id(), utils::consts::TASK_ROOT_TID);
    let task_root = engine
        .runtime()
        .store()
        .tasks()
        .find(&id.id())
        .await
        .unwrap();
    let data: Vars = serde_json::from_str(&task_root.data).unwrap();
    println!("data {}", data);
    assert_eq!(data.get::<i32>("var1").unwrap(), 1);
}
