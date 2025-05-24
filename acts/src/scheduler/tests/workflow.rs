use crate::event::EventAction;
use crate::utils::test::create_proc_signal_with_auto_clomplete;
use crate::{
    Act, Message, MessageState, StmtBuild, TaskState, Vars, Workflow,
    scheduler::tests::create_proc_signal, utils,
};

#[tokio::test]
async fn sch_workflow_setup_set() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", 5))))
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<bool>(&mut workflow, &utils::longid());
    emitter.on_start(move |_e| {
        rx.send(true);
    });

    scher.launch(&proc);
    tx.recv().await;
    proc.print();
    assert_eq!(proc.data().get::<i32>("a").unwrap(), 5);
}

// #[tokio::test]
// async fn sch_workflow_setup_expose() {
//     let mut workflow = Workflow::new()
//         .with_input("a", json!(5))
//         .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", ()))));

//     workflow.print();
//     let (proc, scher, emitter, tx, rx) =
//         create_proc_signal::<Vars>(&mut workflow, &utils::longid());
//     // emitter.reset();
//     emitter.on_complete(move |e| {
//         println!("message: {:?}", e.outputs);
//         rx.send(e.outputs.clone())
//     });
//     scher.launch(&proc);
//     let ret = tx.recv().await;
//     proc.print();
//     assert_eq!(ret.get::<i32>("a").unwrap(), 5);
// }

#[tokio::test]
async fn sch_workflow_setup_msg() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::msg(|msg| {
                msg.with_on(crate::ActEvent::Created)
                    .with_key("msg1")
                    .with_input("a", 5)
            }))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("act") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });
    scher.launch(&proc);
    let messages = tx.recv().await;
    proc.print();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.first().unwrap().key, "msg1");
    assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_created() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::msg(|msg| {
                msg.with_key("msg1")
                    .with_input("a", 5)
                    .with_on(crate::ActEvent::Created)
            }))
        })
        .with_step(|step| step.with_id("step1"));
    workflow.print();
    let (proc, scher, emitter, tx, rx) =
        create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("act") {
            rx.update(|data| data.push(e.inner().clone()));
            rx.close();
        }
    });

    scher.launch(&proc);
    let messages = tx.recv().await;
    proc.print();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.first().unwrap().key, "msg1");
    assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_completed() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::msg(|msg| {
                msg.with_on(crate::ActEvent::Completed)
                    .with_key("msg1")
                    .with_input("a", 5)
            }))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter, sig, s1) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            s1.update(|data| data.push(e.inner().clone()));
            s1.close();
        }
    });

    scher.launch(&proc);
    let messages = sig.recv().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.first().unwrap().key, "msg1");
    assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_step() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::msg(|msg| {
                msg.with_on(crate::ActEvent::Step)
                    .with_key("msg1")
                    .with_input("a", 5)
            }))
        })
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));

    workflow.print();
    let (proc, scher, emitter, sig, s1) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_msg() {
            s1.update(|data| data.push(e.inner().clone()));
        }
    });

    scher.launch(&proc);
    let messages = sig.timeout(1000).await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages.first().unwrap().key, "msg1");
    assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

// #[tokio::test]
// async fn sch_workflow_setup_on_before_update() {
//     let mut workflow = Workflow::new()
//         .with_setup(|setup| {
//             setup.add(Act::on_before_update(|stmts| {
//                 stmts.add(Act::msg(|msg| msg.with_key("msg1").with_input("a", 5)))
//             }))
//         })
//         .with_step(|step| {
//             step.with_id("step1")
//                 .with_act(Act::irq(|req| req.with_key("act1")))
//         })
//         .with_step(|step| {
//             step.with_id("step2")
//                 .with_act(Act::irq(|req| req.with_key("act2")))
//         });

//     workflow.print();
//     let (proc, scher, emitter, sig, s1) =
//         create_proc_signal::<Vec<Message>>(&mut workflow, &utils::longid());
//     let s2 = sig.clone();
//     emitter.on_message(move |e| {
//         if e.is_type("irq") && e.is_state(MessageState::Created) {
//             e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
//                 .unwrap();
//         }

//         if e.is_type("msg") {
//             println!("message: {:?}", e);
//             s1.update(|data| data.push(e.inner().clone()));
//         }
//     });
//     emitter.on_complete(move |_| {
//         s2.close();
//     });
//     scher.launch(&proc);
//     let messages = sig.recv().await;
//     proc.print();
//     assert_eq!(proc.state(), TaskState::Completed);
//     assert_eq!(messages.len(), 2);
//     assert_eq!(messages.first().unwrap().key, "msg1");
//     assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
// }

#[tokio::test]
async fn sch_workflow_setup_on_updated() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::msg(|msg| {
                msg.with_on(crate::ActEvent::Updated)
                    .with_key("msg1")
                    .with_input("a", 5)
            }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|req| req.with_key("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::irq(|req| req.with_key("act2")))
        });

    workflow.print();
    let (proc, scher, emitter, sig, s1) = create_proc_signal_with_auto_clomplete::<Vec<Message>>(
        &mut workflow,
        &utils::longid(),
        false,
    );
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.is_irq() && e.is_state(MessageState::Created) {
            e.do_action(&e.pid, &e.tid, EventAction::Next, &Vars::new())
                .unwrap();
        }

        if e.is_msg() {
            println!("message: {:?}", e);
            s1.update(|data| data.push(e.inner().clone()));
        }
    });

    scher.launch(&proc);
    let messages = sig.timeout(500).await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Completed);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages.first().unwrap().key, "msg1");
    assert_eq!(messages.first().unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

// #[tokio::test]
// async fn sch_workflow_setup_on_catch() {
//     let mut workflow = Workflow::new()
//         .with_id("m1")
//         .with_setup(|stmts| {
//             stmts.add(Act::on_catch(|stmts| {
//                 stmts.add(Catch::new().with_on("err1"))
//             }))
//         })
//         .with_step(|step| {
//             step.with_id("step1")
//                 .with_act(Act::irq(|act| act.with_key("act1")))
//         });

//     workflow.print();
//     let (proc, scher, emitter, tx, _) = create_proc_signal::<()>(&mut workflow, &utils::longid());
//     emitter.on_message(move |e| {
//         println!("message: {:?}", e);
//         if e.is_type("irq") && e.is_state(MessageState::Created) {
//             let options = Vars::new()
//                 .with(consts::ACT_ERR_CODE, "err1")
//                 .with(consts::ACT_ERR_MESSAGE, "");
//             e.do_action(&e.pid, &e.tid, EventAction::Error, &options)
//                 .unwrap();
//         }
//     });
//     scher.launch(&proc);
//     tx.recv().await;
//     proc.print();
//     assert_eq!(proc.state(), TaskState::Completed);
// }

// #[tokio::test]
// async fn sch_workflow_setup_on_timeout() {
//     let mut workflow = Workflow::new()
//         .with_id("m1")
//         .with_setup(|stmts| {
//             stmts.add(Act::on_timeout(|stmts| {
//                 stmts.add(
//                     Timeout::new()
//                         .with_on("1s")
//                         .with_then(|stmts| stmts.add(Act::msg(|act| act.with_key("msg1")))),
//                 )
//             }))
//         })
//         .with_step(|step| {
//             step.with_id("step1")
//                 .with_act(Act::irq(|act| act.with_key("act1")))
//         });

//     workflow.print();
//     let (proc, scher, emitter, tx, rx) = create_proc_signal(&mut workflow, &utils::longid());
//     emitter.on_message(move |e| {
//         println!("message: {:?}", e);
//         if e.is_key("msg1") {
//             rx.send(true);
//         }
//     });
//     scher.launch(&proc);
//     let ret = tx.recv().await;
//     proc.print();
//     assert!(ret);
// }

#[tokio::test]
async fn sch_workflow_hooks_store() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_setup(|stmts| {
            stmts
                .add(Act::msg(|act| {
                    act.with_on(crate::ActEvent::Created).with_key("msg1")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Completed).with_key("msg2")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::BeforeUpdate).with_key("msg3")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Updated).with_key("msg4")
                }))
                .add(Act::msg(|msg| {
                    msg.with_on(crate::ActEvent::Step).with_key("msg5")
                }))
            // .add(Act::on_timeout(|stmts| {
            //     stmts.add(Timeout::new().with_on("2h"))
            // }))
            // .add(Act::on_catch(|stmts| {
            //     stmts.add(Catch::new().with_on("err1"))
            // }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")))
        });

    workflow.print();
    let (proc, rt, emitter, tx, rx) =
        create_proc_signal_with_auto_clomplete(&mut workflow, &utils::longid(), false);
    let cache = rt.cache().clone();
    let pid = proc.id().to_string();
    let rt2 = rt.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_irq() && e.is_state(MessageState::Created) {
            cache.uncache(&pid);
            cache
                .restore(&rt2, |proc| {
                    if let Some(task) = proc.task("$") {
                        rx.update(move |data| *data = task.hooks().len());
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
