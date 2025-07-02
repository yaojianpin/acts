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
    let model = Workflow::new().with_step(|step| step.with_id("step1"));
    let m = model.to_yml();
    assert!(m.is_ok());
}

#[test]
fn model_workflow_to_json_str() {
    let model = Workflow::new().with_step(|step| step.with_id("step1"));
    let m = model.to_json();
    assert!(m.is_ok());
}

#[test]
fn model_workflow_set_id() {
    let mut m = Workflow::new();
    m.set_id("m1");
    assert_eq!(m.id, "m1");
}

#[test]
fn model_workflow_set_vars() {
    let mut m = Workflow::new();
    let mut vars = Vars::new();
    vars.insert("v1".to_string(), 5.into());
    m.set_vars(&vars);
    assert_eq!(m.vars.get_value("v1"), Some(&json!(5)));
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
fn model_workflow_desc() {
    let m = Workflow::new().with_desc("desc1");
    assert_eq!(m.desc, "desc1");
}

#[test]
fn model_workflow_inputs_schema() {
    let schema = json!({ "type": ["string"] });
    let m = Workflow::new().with_inputs(schema.clone());
    assert_eq!(m.inputs, schema);
}

#[test]
fn model_workflow_outputs_schema() {
    let schema = json!({ "type": "object", "properties": { "data": { "type": "string"} } });
    let m = Workflow::new().with_outputs(schema.clone());
    assert_eq!(m.outputs, schema);
}

#[test]
fn model_workflow_on_event() {
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_on(|act| {
            act.with_id("event1")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_on(|act| {
            act.with_id("event2")
                .with_uses("acts.event.manual")
                .with_params_vars(|vars| vars.with("test", 20))
        })
        .with_step(|step| step.with_id("step1"));
    assert_eq!(workflow.on.len(), 2);
}

#[test]
fn model_workflow_set_metadata() {
    let m = Workflow::new()
        .with_metadata("r1", 1)
        .with_metadata("r2", "abc")
        .with_metadata("r3", json!(["a", "b"]))
        .with_metadata("r4", true);

    assert_eq!(m.metadata.get::<i32>("r1").unwrap(), 1);
    assert_eq!(m.metadata.get::<String>("r2").unwrap(), "abc");
    assert_eq!(m.metadata.get::<Vec<String>>("r3").unwrap(), vec!["a", "b"]);
    assert!(m.metadata.get::<bool>("r4").unwrap());
}
