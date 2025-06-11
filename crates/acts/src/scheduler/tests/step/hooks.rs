use crate::event::EventAction;
use crate::{
    Act, Message, MessageState, StmtBuild, Vars, Workflow,
    scheduler::TaskState,
    utils::{self, consts, test::*},
};

#[tokio::test]
async fn sch_step_hooks_created() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(
                    Act::irq(|act| act.with_on(crate::ActEvent::Created).with_key("act1"))
                        .with_id("act1"),
                )
                .add(
                    Act::irq(|act| act.with_on(crate::ActEvent::Created).with_key("act2"))
                        .with_id("act2"),
                )
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
        proc.task_by_nid("act2").first().unwrap().state(),
        TaskState::Interrupt
    );
}

#[tokio::test]
async fn sch_step_hooks_completed() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Completed).with_key("msg1")
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("act1") {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }

        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_hooks_before_update() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::BeforeUpdate).with_key("msg1")
                }))
            })
            .with_act(Act::irq(|act| act.with_key("act1")))
            .with_act(Act::irq(|act| act.with_key("act2")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            if rx.data().len() == 2 {
                rx.close();
            }
        }
        if e.is_irq() && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret.len(), 2);
    assert_eq!(ret.first().unwrap().key, "msg1");
    assert_eq!(ret.get(1).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_hooks_updated() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::irq(|act| act.with_key("act2")))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Updated).with_key("msg1")
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );

    emitter.on_message(move |e| {
        // println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }

        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            if rx.data().len() == 2 {
                rx.close();
            }
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 2);
    assert_eq!(ret.first().unwrap().key, "msg1");
    assert_eq!(ret.get(1).unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_hooks_on_step() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| {
                stmts.add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Step).with_key("msg1")
                }))
            })
            .with_act(Act::irq(|act| act.with_key("act1")))
            .with_act(Act::irq(|act| act.with_key("act2")))
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }

        if e.is_irq() && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_hooks_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_setup(|stmts| stmts.add(Act::irq(|act| act.with_key("act1"))))
            .with_catch(|c| {
                c.with_step(|step| {
                    step.with_id("step2")
                        .with_act(Act::msg(|msg| msg.with_key("msg1")))
                })
            })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() {
            let mut vars = Vars::new();
            vars.set(consts::ACT_ERR_CODE, "100");
            e.do_action(&e.pid, &e.tid, EventAction::Error, &vars)
                .unwrap();
        }

        if e.is_msg() {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let ret = tx.recv().await;
    proc.print();

    assert_eq!(ret.len(), 1);
    assert_eq!(ret.first().unwrap().key, "msg1");
}

#[tokio::test]
async fn sch_step_hooks_store() {
    let mut workflow = Workflow::new().with_id("m1").with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| {
                    act.with_on(crate::ActEvent::Created).with_key("act1")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Completed).with_key("msg2")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::BeforeUpdate).with_key("msg4")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Updated).with_key("msg4")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Step).with_key("msg5")
                }))
        })
    });

    workflow.print();
    let (proc, rt, emitter, tx, rx) = create_proc_signal::<usize>(&mut workflow, &utils::longid());
    let cache = rt.cache().clone();
    let pid = proc.id().to_string();
    let rt2 = rt.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            cache.uncache(&pid);
            cache
                .restore(&rt2, |proc| {
                    if let Some(task) = proc.task_by_nid("step1").first() {
                        rx.update(|data| *data = task.hooks().len());
                    }
                })
                .unwrap();
            rx.close();
        }
    });
    rt.launch(&proc);
    let ret = tx.recv().await;
    proc.print();
    assert_eq!(ret, 5);
}
