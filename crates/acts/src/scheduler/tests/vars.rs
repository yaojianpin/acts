use crate::event::EventAction;
use crate::utils::test::auto_complete;
use crate::{
    Action, MessageState, Variant, VariantTypes, Vars, Workflow,
    utils::{
        self,
        test::{USES_IRQ, USES_SET, create_proc},
    },
};
use serde_json::json;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_inputs() {
    let workflow = Workflow::new().with_var("var1", 10);
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(proc.data().get::<i64>("var1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_outputs_value() {
    let workflow = Workflow::new().with_expose(Variant::create("var1", 10));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    emitter.on_error(move |_| tx_close.close());
    emitter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_outputs_script() {
    let workflow = Workflow::new()
        .with_var("a", json!(10))
        .with_expose(Variant::new().name("var1").r#type(VariantTypes::Number).value(json!(r#"{{ a }}"#)));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    emitter.on_error(move |_| tx_close.close());
    emitter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_default_expose() {
    let workflow = Workflow::new().with_var("var1", 10);

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    emitter.on_error(move |_| tx_close.close());
    emitter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_workflow_expose_options() {
    let workflow = Workflow::new()
        .with_var("var1", 10)
        .with_var("var2", 20)
        .with_expose(Variant::new().name("var1").r#type(VariantTypes::Number));

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    emitter.on_error(move |_| tx_close.close());
    emitter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
    assert_eq!(ret.get::<i64>("var2"), None);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_get_with_script() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("var1", 10)
            .with_var("var2", r#"{{ var1 }}"#)
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .inputs()
            .get_value("var2")
            .unwrap(),
        &json!(10)
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_get_with_not_exists() {
    let workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_var("var2", r#"{{ var1 }}"#));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .inputs()
            .get_value("var2")
            .unwrap(),
        &json!(null)
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_output_only_key_name() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("var1", 10)
            .with_expose(Variant::create("var1", json!(null)))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get_value("var1")
            .unwrap(),
        &json!(10)
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_step_inputs() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_var("var1", 10));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_one_step_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_expose(Variant::create("var1", 10))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_step_default_expose() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1").with_var("var1", 10));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_step_expose_options() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("var1", 10)
            .with_var("var2", 20)
            .with_expose(Variant::create("var1", json!(null)))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var2"),
        None
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_two_steps_outputs() {
    let workflow = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_expose(Variant::create("var1", 10))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_expose(Variant::create("var1", 20))
        });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
    assert_eq!(
        proc.task_by_nid("step2")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        20
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_var("var1", 10))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_nid("b1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_expose(Variant::create("var1", 10))
        })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_default_expose() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_if("true").with_var("var1", 10))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_expose_options() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_var("var1", 10)
                .with_var("var2", 20)
                .with_expose(Variant::create("var1", json!(null)))
        })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("b1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
    assert_eq!(
        proc.task_by_nid("b1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var2"),
        None
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_one_step_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_var("var1", json!(10))
                .with_step(|step| {
                    step.with_id("step1")
                        .with_expose(Variant::create("var1", json!(100)))
                })
        })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        100
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_branch_two_steps_outputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_var("var1", json!(10))
                .with_step(|step| {
                    step.with_id("step1")
                        .with_expose(Variant::create("var1", json!(100)))
                })
                .with_step(|step| {
                    step.with_id("step2")
                        .with_expose(Variant::create("var1", json!(200)))
                })
        })
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        100
    );
    assert_eq!(
        proc.task_by_nid("step2")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        200
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_inputs() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("var1", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(bool::default());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            rx.update(|data| *data = e.inner().inputs.get_value("var1").unwrap() == &json!(10));
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    let ret = tx.recv().await;
    assert!(ret);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_data() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_expose(Variant::create("var1", json!(null)))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_default_expose() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.set("var1", 10);
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    sig.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_expose() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, _) = engine.signal(()).double();
    auto_complete(&engine, &tx);
    let rt = engine.runtime();
    let channel = engine.channel();
    let s = rt.clone();
    channel.on_message(move |e| {
        if e.is_type("act") && e.is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("var1".to_string(), 10.into());
            options.insert("var2".to_string(), 20.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var2")
            .unwrap(),
        20
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_options() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());

    let s = rt.clone();
    emitter.on_message(move |e| {
        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
            && e.inner().is_state(MessageState::Created)
        {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, options);
            s.do_action(&action).unwrap();
        }
    });

    rt.launch(&proc).unwrap();
    sig.recv().await;
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_get_global_vars() {
    let workflow = Workflow::new()
        .with_var("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_expose(Variant::create("var1", json!(null)))
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .find::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_act_inputs_from_step() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("a", json!("abc"))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &tx);
    let rt = engine.runtime();
    let channel = engine.channel();
    channel.on_message(move |e| {
        if e.is_type("act") && e.is_state(MessageState::Created) {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_params("key", "act1")
            .first()
            .unwrap()
            .inputs()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_override_global_vars() {
    let workflow = Workflow::new()
        .with_var("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1")
                .with_expose(Variant::create("var1", json!(null)))
                .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
        });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    proc.task_by_nid("step1")
        .first()
        .unwrap()
        .update_data(&Vars::new().with("a", 10));
    assert_eq!(proc.data().get::<i32>("a").unwrap(), json!(10));
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_override_step_vars() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("a", json!("abc"))
            .with_id("step1")
            .with_expose(Variant::create("var1", json!(null)))
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let tx = sig.clone();
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = tx.clone();
    let tx_close2 = tx.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    proc.print();

    proc.task_by_params("key", "act1")
        .first()
        .unwrap()
        .update_data(&Vars::new().with("a", 10));
    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .find::<i32>("a")
            .unwrap(),
        json!(10)
    );
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_vars_private_vars() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_var("__a", json!("abc"))
            .with_id("step1")
            .with_uses(USES_SET, Vars::new().with("__a", "xyz"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let rx = sig.clone();
    let emitter = engine.channel();
    let tx_close = sig.clone();
    let tx_close2 = sig.clone();
    emitter.on_complete(move |_| tx_close.close());
    emitter.on_error(move |_| tx_close2.close());
    rt.launch(&proc).unwrap();
    rx.recv().await;
    proc.print();

    assert_eq!(
        proc.task_by_nid("step1")
            .first()
            .unwrap()
            .find::<String>("__a")
            .unwrap(),
        "abc"
    );
}
