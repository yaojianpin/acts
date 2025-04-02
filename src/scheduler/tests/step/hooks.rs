use crate::event::EventAction;
use crate::{
  scheduler::{tests::create_proc_signal, TaskState},
  utils::{self, consts},
  Act, Catch, Message, MessageState, StmtBuild, Timeout, Vars, Workflow,
};

#[tokio::test]
async fn sch_step_hooks_created() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts.add(Act::on_created(|stmts| {
                stmts
                    .add(Act::irq(|act| act.with_key("act1")).with_id("act1"))
                    .add(Act::irq(|act| act.with_key("act2")).with_id("act2"))
            }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) = create_proc_signal::<()>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") {
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
                .add(Act::on_completed(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("act1") {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
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
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::irq(|act| act.with_key("act2")))
                .add(Act::on_before_update(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
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
async fn sch_step_hooks_updated() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::irq(|act| act.with_key("act2")))
                .add(Act::on_updated(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());

    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_source("act") && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
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
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::irq(|act| act.with_key("act2")))
                .add(Act::on_step(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg1")))
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
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
async fn sch_step_hooks_error() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1").with_setup(|stmts| {
            stmts
                .add(Act::irq(|act| act.with_key("act1")))
                .add(Act::on_catch(|stmts| {
                    stmts.add(
                        Catch::new()
                            .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1")))),
                    )
                }))
        })
    });

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("irq") {
            let mut vars = Vars::new();
            vars.set(consts::ACT_ERR_CODE, "100");
            e.do_action(&e.pid, &e.tid, EventAction::Error, &vars)
                .unwrap();
        }

        if e.is_type("msg") {
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
                .add(Act::on_created(|stmts| {
                    stmts.add(Act::irq(|act| act.with_key("act1")))
                }))
                .add(Act::on_completed(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg2")))
                }))
                .add(Act::on_before_update(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg3")))
                }))
                .add(Act::on_updated(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg4")))
                }))
                .add(Act::on_step(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_key("msg5")))
                }))
                .add(Act::on_timeout(|stmts| {
                    stmts.add(Timeout::new().with_on("2h"))
                }))
                .add(Act::on_catch(|stmts| {
                    stmts.add(Catch::new().with_on("err1"))
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
        if e.is_type("irq") && e.is_state(MessageState::Created) {
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
    assert_eq!(ret, 7);
}
