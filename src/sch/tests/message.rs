use crate::{
    event::{ActionState, Emitter},
    sch::{Proc, Scheduler},
    utils::{self, consts},
    Action, Engine, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_message_workflow_created() {
    let mut workflow = Workflow::new();
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "workflow" && msg.inner().state() == ActionState::Created {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_workflow_name() {
    let mut workflow = Workflow::new().with_name("my_name");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "workflow" && msg.inner().state() == ActionState::Created {
            assert_eq!(msg.inner().model_name, "my_name");
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_workflow_tag() {
    let mut workflow = Workflow::new().with_tag("my_tag");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "workflow" && msg.inner().state() == ActionState::Created {
            assert_eq!(msg.inner().model_tag, "my_tag");
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_workflow_id() {
    let mut workflow = Workflow::new().with_id("my_id");
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "workflow" && msg.inner().state() == ActionState::Created {
            assert_eq!(msg.inner().model_id, "my_id");
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_time() {
    let mut workflow =
        Workflow::new().with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().state() == ActionState::Created {
            assert!(msg.inner().start_time > 0);
        }

        if msg.inner().state() == ActionState::Completed {
            assert!(msg.inner().end_time > 0);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_job_created() {
    let mut workflow = Workflow::new().with_job(|job| job.with_id("job1"));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "job" && msg.inner().state() == ActionState::Created {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_job_completed() {
    let mut workflow = Workflow::new().with_job(|job| job.with_id("job1"));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "job" && msg.inner().state() == ActionState::Completed {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_step_created() {
    let mut workflow =
        Workflow::new().with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "step" && msg.inner().state() == ActionState::Created {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_step_completed() {
    let mut workflow =
        Workflow::new().with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "step" && msg.inner().state() == ActionState::Completed {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_branch_created() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_branch(|b| b.with_id("b1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "branch" && msg.inner().state() == ActionState::Created {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_branch_completed() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_branch(|b| b.with_id("b1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "branch" && msg.inner().state() == ActionState::Completed {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_branch_skipped() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1").with_step(|step| {
            step.with_id("step1")
                .with_branch(|b| b.with_id("b1").with_if("false"))
        })
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    emitter.on_message(|msg| {
        if msg.inner().r#type == "branch" && msg.inner().state() == ActionState::Skipped {
            assert!(true);
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_act_created() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_act(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);

    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.inner().r#type == "act" && msg.inner().state() == ActionState::Created {
            assert!(true);
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_act_completed() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_act(|act| act.with_id("act1")))
    });
    let id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &id);
    let s = scher.clone();
    emitter.on_message(move |msg| {
        if msg.inner().r#type == "act" && msg.inner().state() == ActionState::Created {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            let action = Action::new(&msg.inner().proc_id, &msg.inner().id, "complete", &options);
            s.do_action(&action).unwrap();
        }
        if msg.inner().r#type == "act" && msg.inner().state() == ActionState::Completed {
            assert!(true);
            s.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
}

#[tokio::test]
async fn sch_message_send() {
    let mut workflow = Workflow::new().with_job(|mut job| {
        job.name = "job1".to_string();
        job.with_step(|step| step.with_name("step1").with_run(r#"act.send("123")"#))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());
    emitter.on_message(move |e| {
        if e.inner().is_type("message") {
            assert_eq!(e.inner().key, "123");
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
}

#[tokio::test]
async fn sch_message_act_inputs_with_step_id() {
    let mut workflow = Workflow::new().with_job(|job| {
        job.with_id("job1")
            .with_step(|step| step.with_id("step1").with_act(|act| act.with_id("test")))
    });
    workflow.id = utils::longid();
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let step_task_id = Arc::new(Mutex::new("".to_string()));
    emitter.on_message(move |e| {
        if e.inner().is_key("step1") {
            *step_task_id.lock().unwrap() = e.inner().id.to_string();
        }
        if e.inner().is_type("act") && e.inner().is_state("created") {
            assert_eq!(
                e.inner()
                    .inputs
                    .get(consts::FOR_ACT_KEY_STEP_NODE_ID)
                    .clone()
                    .unwrap(),
                &json!("step1")
            );
            assert_eq!(
                e.inner()
                    .inputs
                    .get(consts::FOR_ACT_KEY_STEP_TASK_ID)
                    .clone()
                    .unwrap(),
                &json!(*step_task_id.lock().unwrap())
            );
            e.close();
        }
    });

    scher.launch(&proc);
    scher.event_loop().await;
    proc.print();
}

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Proc>, Arc<Scheduler>, Arc<Emitter>) {
    let engine = Engine::new();
    let scher = engine.scher();

    let proc = Arc::new(Proc::new(&pid));
    proc.load(workflow).unwrap();

    let emitter = scher.emitter().clone();
    let s = scher.clone();
    emitter.on_complete(move |p| {
        if p.inner().state.is_completed() {
            s.close();
        }
    });

    let s2 = scher.clone();
    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s2.close();
    });
    (proc, scher, emitter)
}
