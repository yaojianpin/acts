use crate::{
    event::Emitter,
    sch::{Proc, Scheduler},
    utils, Action, Engine, Vars, Workflow,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn sch_vars_workflow_env() {
    let mut workflow = Workflow::new().with_env("var1", 10.into());
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().get("var1").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_workflow_outputs() {
    let mut workflow = Workflow::new().with_output("var1", 10.into());
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().get("var1").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_get_with_script() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_input("var2", r#"${ env.get("var1") }"#.into())
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .inputs()
            .get("var2")
            .unwrap(),
        &json!(10)
    );
}

#[tokio::test]
async fn sch_vars_output_only_key_name() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_input("var1", 10.into())
            .with_output("var1", json!(null))
    });
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .outputs()
            .get("var1")
            .unwrap(),
        &json!(10)
    );
}

#[tokio::test]
async fn sch_vars_step_inputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_input("var1", 10.into()));
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .room()
            .get("var1")
            .unwrap(),
        json!(10)
    );
}

#[tokio::test]
async fn sch_vars_step_outputs() {
    let mut workflow =
        Workflow::new().with_step(|step| step.with_id("step1").with_output("var1", 10.into()));

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().get("var1").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_branch_inputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_branch(|b| b.with_id("b1").with_input("var1", 10.into()))
    });

    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("b1")
            .get(0)
            .unwrap()
            .room()
            .get("var1")
            .unwrap(),
        json!(10)
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
    let (proc, scher, _) = create_proc(&mut workflow, &utils::longid());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(proc.env().get("var1").unwrap(), json!(10));
}

#[tokio::test]
async fn sch_vars_act_inputs() {
    let ret = Arc::new(Mutex::new(false));
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("act1").with_input("var1", 10.into()))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let r = ret.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state("created") {
            *r.lock().unwrap() = e.inner().inputs.get("var1").unwrap() == &json!(10);
            e.close();
        }
    });
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn sch_vars_act_outputs() {
    let mut workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(|act| act.with_id("act1").with_output("var1", json!(null)))
    });
    let (proc, scher, emitter) = create_proc(&mut workflow, &utils::longid());

    let s = scher.clone();
    emitter.on_message(move |e| {
        if e.inner().is_type("act") && e.inner().is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));
            options.insert("var1".to_string(), 10.into());
            let action = Action::new(&e.inner().proc_id, &e.inner().id, "complete", &options);
            s.do_action(&action).unwrap();
        }
    });

    emitter.on_complete(|e| e.close());
    scher.launch(&proc);
    scher.event_loop().await;
    assert_eq!(
        proc.task_by_nid("step1")
            .get(0)
            .unwrap()
            .room()
            .get("var1")
            .unwrap(),
        json!(10)
    );
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
