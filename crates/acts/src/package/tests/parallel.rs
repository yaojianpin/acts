use crate::{
    Act, MessageState, Vars, Workflow,
    event::EventAction,
    scheduler::TaskState,
    utils::{
        self, consts,
        test::{USES_PARALLEL, auto_complete, create_proc},
    },
};
use serde_json::json;
use serial_test::serial;

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn pack_parallel_setup_list() {
    let workflow =
        Workflow::new().with_step(|step| {
            step.with_id("step1").with_uses(USES_PARALLEL, Vars::from(json!({
            "in": ["u1", "u2"],
            "acts": vec![
                Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")).with_id("act1"))
            ]
        })))
        });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).await.unwrap();
    tx.timeout(500).await;
    proc.print();
    let tasks = proc.task_by_nid("act1");
    assert_eq!(tasks.first().unwrap().state(), TaskState::Interrupt);
    assert!(tasks.iter().any(|t| {
        let options = t.options();
        options.get::<String>(consts::ACT_VALUE).unwrap() == "u1"
            && options.get::<i32>(consts::ACT_INDEX).unwrap() == 0
    }));
    assert!(tasks.iter().any(|t| {
        let options = t.options();
        options.get::<String>(consts::ACT_VALUE).unwrap() == "u2"
            && options.get::<i32>(consts::ACT_INDEX).unwrap() == 1
    }));
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_var_exist() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(["u1", "u2"]))
            .with_uses(USES_PARALLEL, Vars::from(json!({
                "in": "${{ a }}",
                "acts": vec![
                    Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")).with_id("act1"))
                ]
            })))
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<Vars>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        let rx = rx.clone();
        let rt = rt.clone();
        async move {
            if e.is_params_key("act1") && e.is_state(MessageState::Created) {
                rx.update(|data| {
                    let vars = e.inputs.get::<Vars>(consts::ACT_OPTIONS_KEY).unwrap();
                    data.push(vars);
                });
                rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                    .await
                    .unwrap();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    let ret = tx.recv().await;
    proc.print();
    let tasks = proc.task_by_nid("act1");
    assert_eq!(tasks.first().unwrap().state(), TaskState::Completed);

    assert!(ret.iter().any(|t| {
        t.get::<String>(consts::ACT_VALUE).unwrap() == "u1"
            && t.get::<i32>(consts::ACT_INDEX).unwrap() == 0
    }));
    assert!(ret.iter().any(|t| {
        t.get::<String>(consts::ACT_VALUE).unwrap() == "u2"
            && t.get::<i32>(consts::ACT_INDEX).unwrap() == 1
    }));
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_in_not_exist() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_PARALLEL,
            Vars::from(json!({
                "in": r#"$("not_exists")"#,
                "acts": vec![
                    Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")))
                ]
            })),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);

    rt.launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_parallel_in_code() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
                .with_uses(USES_PARALLEL, Vars::from(json!({
                    "in": r#"${{ let a = ["u1", "u2"]; let b = ["u3"];let c = [ "u1" ];let d = [ "u3", "u4" ];a.union(b).difference(c).intersection(d) }}"#,
                    "acts": vec![
                        Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")).with_id("act1"))
                    ]
                })))

    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid()).await;
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {e:?}");
        let rx = rx.clone();
        async move {
            if e.is_type("act") {
                rx.close();
            }
        }
    });
    engine.runtime().launch(&proc).await.unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1").first().unwrap().state(),
        TaskState::Interrupt
    );
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .options()
            .get::<String>(consts::ACT_VALUE)
            .unwrap(),
        "u3"
    );
}
