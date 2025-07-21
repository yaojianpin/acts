mod acts;
mod catch;
mod timeout;

use crate::{Act, Step, Vars, Workflow, utils::consts};
use serde_json::json;

#[test]
fn model_step_yml_simple() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.steps.len(), 1);
    assert_eq!(m.steps.first().unwrap().id, "act1");
}

#[test]
fn model_step_yml_vars() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          vars:
            - name: p1
              value: 5
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.steps.len(), 1);
    assert_eq!(m.steps.first().unwrap().id, "act1");

    let step = m.steps.first().unwrap();
    assert_eq!(step.vars.len(), 1);
    assert_eq!(step.vars().get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_step_yml_expose() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          options:
            expose:
                - name: p1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.steps.len(), 1);
    assert_eq!(m.steps.first().unwrap().id, "act1");

    let step = m.steps.first().unwrap();

    let exposes = step.exposes();
    assert_eq!(exposes.len(), 1);
    assert_eq!(exposes.get_value("p1"), Some(&json!(null)));
}

#[test]
fn model_step_id() {
    let step = Step::new().with_id("step1");
    assert_eq!(step.id, "step1");
}

#[test]
fn model_step_name() {
    let step = Step::new().with_name("my name");
    assert_eq!(step.name, "my name");
}

#[test]
fn model_step_desc() {
    let step = Step::new().with_desc("desc1");
    assert_eq!(step.desc, "desc1");
}

#[test]
fn model_step_set_var() {
    let step = Step::new().with_var("p1", json!(5));
    assert_eq!(step.vars.len(), 1);
    assert_eq!(step.vars().get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_step_set_output() {
    let step = Step::new().with_expose("p1", json!(5));

    let options = step.options.get::<Vars>(consts::ACT_EXPOSE).unwrap();
    assert_eq!(options.len(), 1);
    assert!(options.get_value("p1").is_some());
}

#[test]
fn model_step_tag() {
    let step = Step::new().with_tag("tag1");
    assert_eq!(step.tag, "tag1");
}

#[test]
fn model_step_next() {
    let mut step = Step::new();
    assert!(step.next.is_none());

    step = step.with_next("step1");
    assert_eq!(step.next.unwrap(), "step1");
}

#[test]
fn model_step_branches() {
    let mut step = Step::new();
    assert_eq!(step.branches.len(), 0);

    step = step
        .with_branch(|b| b.with_id("b1"))
        .with_branch(|b| b.with_id("b2"));
    assert_eq!(step.branches.len(), 2);
}

#[test]
fn model_step_acts() {
    let mut step = Step::new();
    assert_eq!(step.acts.len(), 0);

    step = step
        .with_act(Act::irq(|act| act.with_key("act1")))
        .with_act(Act::irq(|act| act.with_key("act2")));
    assert_eq!(step.acts.len(), 2);
}

#[test]
fn model_step_set_metadata() {
    let step = Step::new()
        .with_metadata("r1", 1)
        .with_metadata("r2", "abc")
        .with_metadata("r3", json!(["a", "b"]))
        .with_metadata("r4", true);

    assert_eq!(step.metadata.get::<i32>("r1").unwrap(), 1);
    assert_eq!(step.metadata.get::<String>("r2").unwrap(), "abc");
    assert_eq!(
        step.metadata.get::<Vec<String>>("r3").unwrap(),
        vec!["a", "b"]
    );
    assert!(step.metadata.get::<bool>("r4").unwrap());
}
