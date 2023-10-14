mod r#for;

use crate::{
    event::{Action, ActionState, Emitter},
    sch::{Proc, Scheduler},
    utils, Engine, Executor, Manager, Vars, Workflow,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn sch_act_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("u1"))
            })
        })
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let r = ret.clone();
    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            if e.inner().state() == ActionState::Created {
                let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
                if let Err(err) = s.do_action(&action) {
                    println!("error: {}", err);
                    *r.lock().unwrap() = false;
                } else {
                    *r.lock().unwrap() = true;
                }
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;

    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_normal() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1")
            .with_step(|step| {
                step.with_name("step1").with_act(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
            .with_step(|step| {
                step.with_name("step2").with_act(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
    });

    workflow.print();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    let s = scher.clone();
    let r = ret.clone();

    let act_id = Arc::new(Mutex::new(None));
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;

            if uid == "a" && e.inner().state == ActionState::Created.to_string() {
                if *count == 0 {
                    *act_id.lock().unwrap() = Some(tid.to_string());

                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!(uid.to_string()));

                    let action = Action::new(&e.inner().proc_id, tid, "complete", &options);
                    s.do_action(&action).unwrap();
                } else {
                    *r.lock().unwrap() = true;
                    s.close();
                }
                *count += 1;
            } else if uid == "b" && e.inner().state == ActionState::Created.to_string() {
                // cancel the b's task by a
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!("a".to_string()));

                // get the completed act id in previous step
                let act_id = &*act_id.lock().unwrap();
                let aid = act_id.as_deref().unwrap();
                let action = Action::new(&e.inner().proc_id, aid, "cancel", &options);
                s.do_action(&action).unwrap();
            }
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_back() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1")
            .with_step(|step| {
                step.with_id("step1").with_name("step1").with_act(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
            .with_step(|step| {
                step.with_name("step2").with_act(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        let msg = e.inner();
        if msg.is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = msg.inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &msg.id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&msg.proc_id, tid, "complete", &options);
                s.do_action(&action).unwrap();
            } else if uid == "b" {
                if msg.state() == ActionState::Created {
                    let mut options = Vars::new();
                    options.insert("uid".to_string(), json!("b".to_string()));
                    options.insert("to".to_string(), json!("step1".to_string()));
                    let action = Action::new(&msg.proc_id, tid, "back", &options);
                    s.do_action(&action).unwrap();
                }
            } else if msg.state() == ActionState::Created && uid == "a" && *count > 0 {
                *r.lock().unwrap() = uid == "a";
                s.close();
            }

            *count += 1;
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_abort() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1")
            .with_step(|step| {
                step.with_id("step1").with_name("step1").with_act(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
            .with_step(|step| {
                step.with_name("step2").with_act(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let message = Action::new(&e.inner().proc_id, tid, "abort", &options);
                s.do_action(&message).unwrap();
            }
            *count += 1;
        }
    });

    emitter.on_complete(move |e| {
        *r.lock().unwrap() = e.inner().state.is_abort();
        e.close();
    });
    scher.launch(&proc);
    scher.event_loop().await;
    let result = *ret.lock().unwrap();
    assert!(result);
}

#[tokio::test]
async fn sch_act_submit() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state("created") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "submit", &options);
                s.do_action(&action).unwrap();
            }
        }

        if e.inner().is_type("act") && e.inner().is_state("submitted") {
            *r.lock().unwrap() = true;
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_skip() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter, _, _) = create_proc2(&mut workflow, &pid);
    let s = scher.clone();
    let r = ret.clone();
    let task_id = Arc::new(Mutex::new("".to_string()));
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            if uid == "a" && e.inner().state() == ActionState::Created {
                *task_id.lock().unwrap() = e.inner().id.clone();

                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, &e.inner().id, "skip", &options);
                s.do_action(&action).unwrap();
            }

            // check the skipped state
            if e.inner().id == *task_id.lock().unwrap() && e.inner().state() == ActionState::Skipped
            {
                *r.lock().unwrap() = true;
            }
        }
    });

    emitter.on_complete(move |e| {
        e.close();
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_not_support_action() {
    let ret = Arc::new(Mutex::new(false));
    let count = Arc::new(Mutex::new(0));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let mut count = count.lock().unwrap();
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            if uid == "a" && *count == 0 {
                let mut options = Vars::new();
                options.insert("uid".to_string(), json!(uid.to_string()));

                let action = Action::new(&e.inner().proc_id, tid, "not_support", &options);
                *r.lock().unwrap() = s.do_action(&action).is_err();
                s.close();
            }
            *count += 1;
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_next_by_complete_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1")
            .with_step(|step| {
                step.with_id("step1").with_name("step1").with_act(|act| {
                    act.with_id("fn1")
                        .with_name("fn 1")
                        .with_input("uid", json!("a"))
                })
            })
            .with_step(|step| {
                step.with_name("step2").with_act(|act| {
                    act.with_id("fn2")
                        .with_name("fn 2")
                        .with_input("uid", json!("b"))
                })
            })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, tid, "complete", &options);
            s.do_action(&action).unwrap();

            // action again
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_cancel_by_running_state() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_id(&utils::longid()).with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, "w1");

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();
            let tid = &e.inner().id;
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, tid, "cancel", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_complete() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();

            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_ok();

            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_no_outputs_key_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_output("abc", json!(null))
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            let uid = e.inner().inputs.get("uid").unwrap().as_str().unwrap();

            // create options that not satisfy the outputs
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!(uid.to_string()));

            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_no_uid_key_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_output("abc", json!(null))
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_proc_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new("no_exist_proc_id", &e.inner().id, "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

#[tokio::test]
async fn sch_act_do_action_msg_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_name("job1").with_step(|step| {
            step.with_id("step1").with_name("step1").with_act(|act| {
                act.with_id("fn1")
                    .with_name("fn 1")
                    .with_input("uid", json!("a"))
            })
        })
    });

    let pid = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &pid);

    let s = scher.clone();
    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().state() == ActionState::Created && e.inner().r#type == "act" {
            // create options that not contains uid key
            let options = Vars::new();
            let action = Action::new(&e.inner().proc_id, "no_exist_msg_id", "complete", &options);
            *r.lock().unwrap() = s.do_action(&action).is_err();
            s.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    assert!(*ret.lock().unwrap());
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();
    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow);

    let emitter = scher.emitter().clone();
    let s = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s.close();
    });

    (proc, scher, emitter)
}

fn create_proc2(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Proc>,
    Arc<Scheduler>,
    Arc<Emitter>,
    Arc<Executor>,
    Arc<Manager>,
) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow);

    let emitter = scher.emitter().clone();
    let executor = engine.executor().clone();
    let manager = engine.manager().clone();
    let s = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s.close();
    });
    (proc, scher, emitter, executor, manager)
}
