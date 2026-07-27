use crate::{Branch, Variant, Workflow};
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
fn model_branch_vars() {
    let b = Branch::new().with_var("p1", json!(5));
    assert_eq!(b.vars.len(), 1);
    assert_eq!(b.vars().get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_branch_outputs() {
    let b = Branch::new().with_expose(Variant::create("p1", json!(5)));

    let options = b.exposes();
    assert!(options.get_value("p1").is_some());
}

#[test]
fn model_branch_tag() {
    let b = Branch::new().with_option("tag", "tag1");
    assert_eq!(b.options.get::<String>("tag").unwrap(), "tag1");
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

#[test]
fn model_step_options() {
    let mut b = Branch::new();
    assert!(b.options.is_empty());

    b = b.with_option("max_limit", 5);
    assert_eq!(b.options.get::<i32>("max_limit").unwrap(), 5);
}

#[test]
fn model_branch_set_metadata() {
    let b = Branch::new()
        .with_metadata("r1", 1)
        .with_metadata("r2", "abc")
        .with_metadata("r3", json!(["a", "b"]))
        .with_metadata("r4", true);

    assert_eq!(b.metadata.get::<i32>("r1").unwrap(), 1);
    assert_eq!(b.metadata.get::<String>("r2").unwrap(), "abc");
    assert_eq!(b.metadata.get::<Vec<String>>("r3").unwrap(), vec!["a", "b"]);
    assert!(b.metadata.get::<bool>("r4").unwrap());
}

#[test]
fn model_branch_yml_vars() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          branches:
            - id: b1
              if: true
              vars:
                - name: p1
                  value: 5

    "#;
    let m = Workflow::from_yml(text).unwrap();

    let step = m.steps.first().unwrap();
    let barnch = step.branches.first().unwrap();
    assert_eq!(barnch.vars.len(), 1);
    assert_eq!(barnch.vars().get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_branch_yml_expose() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          branches:
            - id: b1
              if: true
              exposes:
                - name: p1

    "#;

    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let barnch = step.branches.first().unwrap();
    let exposes = barnch.exposes();
    assert_eq!(exposes.len(), 1);
    assert_eq!(exposes.get_value("p1"), Some(&json!(null)));
}

#[test]
fn model_branch_with_expose() {
    let b = Branch::new().with_expose(Variant::create("v1", 0));
    assert!(b.exposes().contains_key("v1"));
}
