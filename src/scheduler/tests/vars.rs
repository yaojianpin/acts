use crate::event::EventAction;
use crate::{
  scheduler::tests::create_proc_signal,
  utils::{self, consts},
  Act, Action, MessageState, Vars, Workflow,
};
use serde_json::json;

#[tokio::test]
async fn sch_vars_workflow_inputs() {
    let mut workflow = Workflow::new().with_input("var1", 10.into());
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(proc.data().get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_workflow_outputs_value() {
    let mut workflow = Workflow::new().with_output("var1", 10.into());
    let (proc, scher, emiter, tx, rx) = create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    // emiter.reset();
    emiter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_workflow_outputs_script() {
    let mut workflow = Workflow::new()
        .with_input("a", json!(10))
        .with_output("var1", json!(r#"${ $("a") }"#));
    let (proc, scher, emiter, tx, rx) = create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    // emiter.reset();
    emiter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_workflow_default_outputs() {
    let mut workflow = Workflow::new()
        .with_env(consts::ACT_DEFAULT_OUTPUTS, json!(["var1"]))
        .with_input("var1", 10.into());

    let (proc, scher, emiter, tx, rx) = create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    // emiter.reset();
    emiter.on_complete(move |e| {
        rx.send(e.outputs.clone());
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.get::<i64>("var1").unwrap(), 10);
}

#[tokio::test]
async fn sch_vars_get_with_script() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_input("var2", r#"${ $("var1") }"#.into())
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_get_with_not_exists() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var2", r#"${ $("var1") }"#.into())
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_output_only_key_name() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_output("var1", json!(null))
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_step_inputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_input("var1", 10.into()));
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_one_step_outputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_output("var1", 10.into()));

    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_step_default_outputs() {
    let mut workflow = Workflow::new()
        .with_env(consts::ACT_DEFAULT_OUTPUTS, json!(["var1"]))
        .with_step(|step| step.with_id("step1").with_input("var1", 10.into()));
    let (proc, scher, _, tx, _) = create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_two_steps_outputs() {
    let mut workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_output("var1", 10.into()))
        .with_step(|step| step.with_id("step2").with_output("var1", 20.into()));

    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_branch_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_input("var1", 10.into()))
    });

    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_branch_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_output("var1", 10.into())
        })
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_branch_default_outputs() {
    let mut workflow = Workflow::new()
        .with_env(consts::ACT_DEFAULT_OUTPUTS, json!(["var1"]))
        .with_step(|step| {
            step.with_id("step1").with_branch(|b| {
                b.with_id("b1")
                    .with_if("true")
                    .with_input("var1", 10.into())
            })
        });
    let (proc, scher, _, tx, _) = create_proc_signal::<Vars>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_branch_one_step_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_input("var1", json!(10))
                .with_step(|step| step.with_id("step1").with_output("var1", json!(100)))
        })
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_branch_two_steps_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_branch(|b| {
            b.with_id("b1")
                .with_if("true")
                .with_input("var1", json!(10))
                .with_step(|step| step.with_id("step1").with_output("var1", json!(100)))
                .with_step(|step| step.with_id("step2").with_output("var1", json!(200)))
        })
    });
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn sch_vars_act_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1").with_input("var1", 10)))
    });
    let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            rx.update(|data| *data = e.inner().inputs.get_value("var1").unwrap() == &json!(10));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn sch_vars_act_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_act(
            Act::irq(|act| act.with_key("act1").with_ret("var1", json!(null))).with_id("act1"),
        )
    });
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_act_default_outputs() {
    let mut workflow = Workflow::new()
        .with_env(consts::ACT_DEFAULT_OUTPUTS, json!(["var1"]))
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
        });
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .outputs()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_act_options() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")).with_id("act1"))
    });
    let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.is_key("act1") && e.inner().is_state(MessageState::Created) {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().pid, &e.inner().tid, EventAction::Next, &options);
            s.do_action(&action).unwrap();
        }
    });

    scher.launch(&proc);
    tx.recv().await;
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .data()
            .get::<i64>("var1")
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn sch_vars_get_global_vars() {
    let mut workflow = Workflow::new()
        .with_input("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1").with_act(Act::irq(|act| {
                act.with_key("act1").with_ret("var1", json!(null))
            }))
        });
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    scher.launch(&proc);
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

// #[tokio::test]
// async fn sch_vars_act_outputs_from_step() {
//     let mut workflow = Workflow::new().with_step(|step| {
//         step.with_id("step1")
//             .with_input("a", json!("abc"))
//             .with_act(
//                 Act::irq(|act| act.with_key("act1").with_ret("a", json!(null))).with_id("act1"),
//             )
//     });
//     let (process, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
//     emitter.on_message(move |e| {
//         println!("message: {e:?}");
//         if e.inner().is_source("act") && e.inner().is_state(MessageState::Created") {
//             rx.close();
//         }
//     });
//     scher.launch(&process);
//     tx.recv().await;
//     process.print();
//     assert_eq!(
//         process.task_by_nid("act1")
//             .first()
//             .unwrap()
//             .outputs()
//             .get::<String>("a")
//             .unwrap(),
//         "abc"
//     );
// }

#[tokio::test]
async fn sch_vars_act_inputs_from_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("a", json!("abc"))
            .with_act(
                Act::irq(|act| {
                    act.with_key("act1")
                        .with_input("a", json!(r#"${ $("a") }"#))
                })
                .with_id("act1"),
            )
    });
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(
        proc.task_by_nid("act1")
            .first()
            .unwrap()
            .inputs()
            .get::<String>("a")
            .unwrap(),
        "abc"
    );
}

#[tokio::test]
async fn sch_vars_override_global_vars() {
    let mut workflow = Workflow::new()
        .with_input("a", json!("abc"))
        .with_step(|step| {
            step.with_id("step1").with_act(Act::irq(|act| {
                act.with_key("act1").with_ret("var1", json!(null))
            }))
        });
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    proc.task_by_nid("step1")
        .first()
        .unwrap()
        .update_data(&Vars::new().with("a", 10));
    assert_eq!(proc.data().get::<i32>("a").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_override_step_vars() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_input("a", json!("abc"))
            .with_id("step1")
            .with_act(
                Act::irq(|act| act.with_key("act1").with_ret("var1", json!(null))).with_id("act1"),
            )
    });
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_source("act") && e.inner().is_state(MessageState::Created) {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
    proc.print();

    proc.task_by_nid("act1")
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
