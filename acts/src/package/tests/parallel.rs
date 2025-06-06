use crate::{
    Act, MessageState, StmtBuild, Vars, Workflow,
    event::EventAction,
    scheduler::TaskState,
    utils::{
        self, consts,
        test::{create_proc_signal, create_proc_signal_with_auto_clomplete},
    },
};
use serde_json::json;

#[tokio::test]
async fn pack_parallel_setup_list() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup.add(Act::parallel(json!({
                "in": ["u1", "u2"],
                "acts": vec![
                    Act::irq(|act| act.with_key("act1").with_id("act1"))
                ]
            })))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal_with_auto_clomplete::<()>(&mut workflow, &utils::longid(), false);
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
    tx.recv().await;
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

#[tokio::test]
async fn pack_parallel_var_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::set(Vars::new().with("a", ["u1", "u2"])))
            .with_act(Act::parallel(json!({
                "in": r#"{{ a }}"#,
                "acts": vec![
                    Act::irq(|act| act.with_key("act1").with_id("act1"))
                ]
            })))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Vars>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            rx.update(|data| {
                let vars = e.inputs.get::<Vars>(consts::ACT_OPTIONS_KEY).unwrap();
                data.push(vars);
            });
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
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

#[tokio::test]
async fn pack_parallel_setup_var_not_exist() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts.add(Act::parallel(json!({
                "in": r#"$("not_exists")"#,
                "acts": vec![
                    Act::irq(|act| act.with_key("act1"))
                ]
            })))
        })
    });

    workflow.print();
    let (proc, scher, _, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert!(proc.state().is_error());
}

#[tokio::test]
async fn pack_parallel_setup_code() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|setup| {
            setup
                .add(Act::set(Vars::new().with("a", ["u1", "u2"])))
                .add(Act::parallel(json!({
                    "in": r#"{{ let b = ["u3"];let c = [ "u1" ];let d = [ "u3", "u4" ];a.union(b).difference(c).intersection(d) }}"#,
                    "acts": vec![
                        Act::irq(|act| act.with_key("act1").with_id("act1"))
                    ]
                })))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("act") {
            rx.close();
        }
    });
    scher.launch(&proc);
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
