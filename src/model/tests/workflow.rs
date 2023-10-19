use crate::{Vars, Workflow};
use serde_json::json;

#[test]
fn model_workflow_from_yml_str() {
    let text = r#"
    name: workflow
    id: m1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.id, "m1");
    assert_eq!(m.name, "workflow");
}

#[test]
fn model_workflow_from_json_str() {
    let text = r#"
    {
        "name": "workflow",
        "id": "m1"
    }
    "#;
    let m = Workflow::from_json(text).unwrap();
    assert_eq!(m.id, "m1");
    assert_eq!(m.name, "workflow");
}

#[test]
fn model_workflow_to_yml_str() {
    let model =
        Workflow::new().with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    let m = model.to_yml();
    assert_eq!(m.is_ok(), true);
}

#[test]
fn model_workflow_to_json_str() {
    let model =
        Workflow::new().with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    let m = model.to_json();
    assert_eq!(m.is_ok(), true);
}

#[test]
fn model_workflow_set_id() {
    let mut m = Workflow::new();
    m.set_id("m1");
    assert_eq!(m.id, "m1");
}

#[test]
fn model_workflow_set_env() {
    let mut m = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("v1".to_string(), 5.into());
    m.set_env(&vars);
    assert_eq!(m.env.get("v1"), Some(&json!(5)));
}

#[test]
fn model_workflow_name() {
    let m = Workflow::new().with_name("my name");
    assert_eq!(m.name, "my name");
}

#[test]
fn model_workflow_jobs() {
    let m = Workflow::new()
        .with_job(|job| job.with_id("job1"))
        .with_job(|job| job.with_id("job2"));
    assert_eq!(m.jobs.len(), 2);
}

#[test]
fn model_workflow_tag() {
    let m = Workflow::new().with_tag("tag1");
    assert_eq!(m.tag, "tag1");
}

#[test]
fn model_workflow_actions() {
    let m = Workflow::new()
        .with_action(|action| action.with_id("a1"))
        .with_action(|action| action.with_id("a2"));
    assert_eq!(m.actions.len(), 2);
    assert!(m.action("a1").is_some());
}

#[test]
fn model_workflow_actions_with_on() {
    let m = Workflow::new().with_action(|action| {
        action.with_id("a1").with_on(|on| {
            on.with_state("created")
                .with_nid("step1")
                .with_nkind("step")
        })
    });
    assert!(m.action("a1").is_some());
    assert_eq!(m.action("a1").unwrap().on.get(0).unwrap().state, "created");
    assert_eq!(
        m.action("a1")
            .unwrap()
            .on
            .get(0)
            .unwrap()
            .nkind
            .as_ref()
            .unwrap(),
        "step"
    );
    assert_eq!(
        m.action("a1")
            .unwrap()
            .on
            .get(0)
            .unwrap()
            .nid
            .as_ref()
            .unwrap(),
        "step1"
    );
}

#[test]
fn model_workflow_valid() {
    let m = Workflow::new()
        .with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")))
        .with_job(|job| job.with_id("job1").with_step(|step| step.with_id("step1")));
    assert_eq!(m.valid().is_err(), true);
}
