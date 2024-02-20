use crate::{
    sch::tests::create_proc, utils, Act, Catch, StmtBuild, TaskState, Timeout, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_workflow_setup_set() {
    let mut workflow = Workflow::new()
        .with_setup(|setup| setup.add(Act::set(Vars::new().with("a", 5))))
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    assert_eq!(proc.env().root().get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_expose() {
    let ret = Arc::new(Mutex::new(Vars::new()));
    let mut workflow = Workflow::new()
        .with_input("a", json!(5))
        .with_setup(|setup| setup.add(Act::expose(Vars::new().with("a", ()))));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_complete(move |e| {
        println!("message: {:?}", e.outputs());
        *r.lock().unwrap() = e.outputs().clone();
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();

    assert_eq!(ret.lock().unwrap().get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_msg() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| setup.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5))))
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_created() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::on_created(|stmts| {
                stmts.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5)))
            }))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(proc.state(), TaskState::Running);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_completed() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::on_completed(|stmts| {
                stmts.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5)))
            }))
        })
        .with_step(|step| step.with_id("step1"));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(proc.state(), TaskState::Success);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_step() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::on_step(|stmts| {
                stmts.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5)))
            }))
        })
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("msg") {
            m.lock().unwrap().push(e.inner().clone());
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(proc.state(), TaskState::Success);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_before_update() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::on_before_update(|stmts| {
                stmts.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5)))
            }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|req| req.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|req| req.with_id("act2")))
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        if e.is_type("req") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
            println!("message: {:?}", e);
            m.lock().unwrap().push(e.inner().clone());
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(proc.state(), TaskState::Success);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_updated() {
    let messages = Arc::new(Mutex::new(vec![]));
    let mut workflow = Workflow::new()
        .with_setup(|setup| {
            setup.add(Act::on_updated(|stmts| {
                stmts.add(Act::msg(|msg| msg.with_id("msg1").with_input("a", 5)))
            }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|req| req.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|req| req.with_id("act2")))
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let m = messages.clone();
    emitter.on_message(move |e| {
        if e.is_type("req") && e.is_state("created") {
            e.do_action(&e.proc_id, &e.id, "complete", &Vars::new())
                .unwrap();
        }

        if e.is_type("msg") {
            println!("message: {:?}", e);
            m.lock().unwrap().push(e.inner().clone());
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    let messages = messages.lock().unwrap();
    assert_eq!(proc.state(), TaskState::Success);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages.get(0).unwrap().key, "msg1");
    assert_eq!(messages.get(0).unwrap().inputs.get::<i32>("a").unwrap(), 5);
}

#[tokio::test]
async fn sch_workflow_setup_on_catch() {
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_setup(|stmts| {
            stmts.add(Act::on_error_catch(|stmts| {
                stmts.add(Catch::new().with_err("err1"))
            }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") && e.is_state("created") {
            let options = Vars::new().with("err_code", "err1");
            e.do_action(&e.proc_id, &e.id, "error", &options).unwrap();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(proc.state(), TaskState::Success);
}

#[tokio::test]
async fn sch_workflow_setup_on_timeout() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_setup(|stmts| {
            stmts.add(Act::on_timeout(|stmts| {
                stmts.add(
                    Timeout::new()
                        .with_on("1s")
                        .with_then(|stmts| stmts.add(Act::msg(|act| act.with_id("msg1")))),
                )
            }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_key("msg1") {
            *r.lock().unwrap() = true;
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_workflow_hooks_store() {
    let ret = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new()
        .with_id("m1")
        .with_setup(|stmts| {
            stmts
                .add(Act::on_created(|stmts| {
                    stmts.add(Act::msg(|act| act.with_id("msg1")))
                }))
                .add(Act::on_completed(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_id("msg2")))
                }))
                .add(Act::on_before_update(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_id("msg3")))
                }))
                .add(Act::on_updated(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_id("msg4")))
                }))
                .add(Act::on_step(|stmts| {
                    stmts.add(Act::msg(|msg| msg.with_id("msg5")))
                }))
                .add(Act::on_timeout(|stmts| {
                    stmts.add(Timeout::new().with_on("2h"))
                }))
                .add(Act::on_error_catch(|stmts| {
                    stmts.add(Catch::new().with_err("err1"))
                }))
        })
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let cache = scher.cache().clone();
    let pid = proc.id().clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        println!("message: {:?}", e);
        if e.is_type("req") && e.is_state("created") {
            cache.uncache(&pid);
            cache
                .restore(|proc| {
                    if let Some(task) = proc.task("$") {
                        *r.lock().unwrap() = task.hooks().len();
                    }
                })
                .unwrap();
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert_eq!(*ret.lock().unwrap(), 7);
}
