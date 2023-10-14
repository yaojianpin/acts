use crate::Step;
use serde_json::json;

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
fn model_step_inputs() {
    let step = Step::new().with_input("p1", json!(5));
    assert_eq!(step.inputs.len(), 1);
    assert_eq!(step.inputs.get("p1"), Some(&json!(5)));
}

#[test]
fn model_step_outputs() {
    let step = Step::new().with_output("p1", json!(5));
    assert_eq!(step.outputs.len(), 1);
    assert!(step.outputs.get("p1").is_some());
}

#[test]
fn model_step_tag() {
    let step = Step::new().with_tag("tag1");
    assert_eq!(step.tag, "tag1");
}

#[test]
fn model_step_run() {
    let step = Step::new().with_run(r#"print("run")"#);
    assert!(step.run.is_some());
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
        .with_act(|a| a.with_id("act1"))
        .with_act(|a| a.with_id("act2"));
    assert_eq!(step.acts.len(), 2);
}

