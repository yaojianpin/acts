use crate::{Act, StmtBuild, Vars, Workflow};
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
    let model = Workflow::new().with_step(|step| step.with_id("step1"));
    let m = model.to_yml();
    assert_eq!(m.is_ok(), true);
}

#[test]
fn model_workflow_to_json_str() {
    let model = Workflow::new().with_step(|step| step.with_id("step1"));
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
fn model_workflow_set_input() {
    let mut m = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("v1".to_string(), 5.into());
    m.set_inputs(&vars);
    assert_eq!(m.inputs.get_value("v1"), Some(&json!(5)));
}

#[test]
fn model_workflow_set_env() {
    let mut m = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("v1".to_string(), 5.into());
    m.set_env(&vars);
    assert_eq!(m.env.get_value("v1"), Some(&json!(5)));
}

#[test]
fn model_workflow_name() {
    let m = Workflow::new().with_name("my name");
    assert_eq!(m.name, "my name");
}

#[test]
fn model_workflow_steps() {
    let m = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step2"));
    assert_eq!(m.steps.len(), 2);
}

#[test]
fn model_workflow_tag() {
    let m = Workflow::new().with_tag("tag1");
    assert_eq!(m.tag, "tag1");
}

#[test]
fn model_workflow_setup_build() {
    let m = Workflow::new().with_setup(|stmts| {
        stmts
            .add(Act::msg(|msg| msg.with_id("msg1")))
            .add(Act::set(Vars::new().with("a", 5)))
    });
    assert_eq!(m.setup.len(), 2);
}

#[test]
fn model_workflow_setup_parse() {
    let text = r#"
    name: workflow
    id: m1
    setup:
       - !msg
         id: msg1
       - !set
         a: 6
       - !on_created
         - !msg
           id: msg2
       - !on_completed
           - !msg
             id: msg3
       - !on_step
           - !msg
             id: msg3
       - !on_before_update
           - !msg
             id: msg3
       - !on_updated
           - !msg
             id: msg3
       - !on_step
           - !msg
             id: msg3
       - !expose
         out:
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.setup.len(), 9);
}
