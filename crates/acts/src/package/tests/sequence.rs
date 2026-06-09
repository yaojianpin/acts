use serde_json::json;

use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{
    Act, MessageState, Vars, Workflow,
    utils::{
        self, consts,
        test::{USES_SEQUENCE, create_proc},
    },
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_sequence_chain_list() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SEQUENCE,
            Vars::from(json!({
                "in": ["u1", "u2"],
                "acts": vec![
                    Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")))
                ]
            })),
        )
    });

    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<String>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rx.update(|data| {
                let vars = e.inputs.get::<Vars>(consts::ACT_OPTIONS_KEY).unwrap();
                data.push(vars.get::<String>(consts::ACT_VALUE).unwrap());
            });
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, ["u1", "u2"]);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_sequence_chain_order() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SEQUENCE,
            Vars::from(json!({
                "in": ["u1", "u2"],
                "acts": vec![
                    Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")))
                ]
            })),
        )
    });

    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<i64>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();

    channel.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rx.update(|data| data.push(e.start_time));
            std::thread::sleep(std::time::Duration::from_secs(1));
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    let time1 = ret.first().unwrap();
    let time2 = ret.get(1).unwrap();
    assert!(time2 - time1 > 1000);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_sequence_chain_var() {
    let main = Workflow::new().with_id("main").with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!(["u1", "u2"]))
            .with_uses(
                USES_SEQUENCE,
                Vars::from(json!({
                    "in": r#"{{ a }}"#,
                    "acts": vec![
                        Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")))
                    ]
                })),
            )
    });

    main.print();
    let (engine, proc) = create_proc(&main, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(Vec::<String>::default()).double();
    auto_complete(&engine, &rx);
    let channel = engine.channel();
    channel.on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rx.update(|data| {
                let vars = e.inputs.get::<Vars>(consts::ACT_OPTIONS_KEY).unwrap();
                data.push(vars.get::<String>(consts::ACT_VALUE).unwrap());
            });
            rt.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });

    engine.runtime().launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, ["u1", "u2"]);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn pack_sequence_chain_var_not_exist() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_uses(
            USES_SEQUENCE,
            Vars::from(json!({
                "in": r#"$("a")"#,
                "acts": vec![
                    Act::irq(|act| act.with_params_vars(|v| v.with("key", "act1")))
                ]
            })),
        )
    });

    workflow.print();
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}
