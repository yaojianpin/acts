use crate::Branch;
use serde_json::json;

#[test]
fn model_branch_id() {
    let b = Branch::new().with_id("b1");
    assert_eq!(b.id, "b1");
}

#[test]
fn model_branch_name() {
    let b = Branch::new().with_name("my name");
    assert_eq!(b.name, "my name");
}

#[test]
fn model_branch_inputs() {
    let b = Branch::new().with_input("p1", json!(5));
    assert_eq!(b.inputs.len(), 1);
    assert_eq!(b.inputs.get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_branch_outputs() {
    let b = Branch::new().with_output("p1", json!(5));
    assert_eq!(b.outputs.len(), 1);
    assert!(b.outputs.get_value("p1").is_some());
}

#[test]
fn model_branch_tag() {
    let b = Branch::new().with_tag("tag1");
    assert_eq!(b.tag, "tag1");
}

#[test]
fn model_branch_run() {
    let b = Branch::new().with_run(r#"print("run")"#);
    assert!(b.run.is_some());
}

#[test]
fn model_branch_else() {
    let mut b = Branch::new();
    assert!(!b.r#else);

    b = b.with_else(true);
    assert!(b.r#else);
}

#[test]
fn model_branch_needs() {
    let mut b = Branch::new();
    assert_eq!(b.needs.len(), 0);

    b = b.with_need("b1");
    assert!(b.needs.contains(&"b1".to_string()));
}

#[test]
fn model_branch_next() {
    let mut b = Branch::new();
    assert!(b.next.is_none());

    b = b.with_next("step1");
    assert_eq!(b.next.unwrap(), "step1");
}

#[test]
fn model_branch_steps() {
    let mut b = Branch::new();
    assert_eq!(b.steps.len(), 0);

    b = b
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));
    assert_eq!(b.steps.len(), 2);
}
