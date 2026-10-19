use crate::{ActSchema, Variant, Vars, Workflow, model::var::VariantTypes};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[test]
fn model_workflow_from_yml_str_name_id() {
    let text = r#"
    name: workflow
    id: m1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.id, "m1");
    assert_eq!(m.name, "workflow");
}

#[test]
fn model_workflow_from_yml_str_env() {
    let text = r#"
    env:
        - name: a
          value: 10
        - name: b
          value: abc
        - name: c
          value: 
            - 1
            - 2
        - name: d
          value: 
            v1: 1
            v2: 2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let env = m.env();

    #[derive(Serialize, Deserialize, Clone)]
    struct Obj {
        v1: i32,
        v2: i32,
    }
    assert_eq!(env.get::<i32>("a").unwrap(), 10);
    assert_eq!(env.get::<String>("b").unwrap(), "abc");
    assert_eq!(env.get::<Vec<i32>>("c").unwrap(), vec![1, 2]);

    let obj = env.get::<Obj>("d").unwrap();
    assert_eq!(obj.v1, 1);
    assert_eq!(obj.v2, 2);
}

#[test]
fn model_workflow_from_yml_str_vars() {
    let text = r#"
    vars:
        - name: a
          value: 10
        - name: b
          value: abc
        - name: c
          value: 
            - 1
            - 2
        - name: d
          value: 
            v1: 1
            v2: 2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let vars = m.vars();

    #[derive(Serialize, Deserialize, Clone)]
    struct Obj {
        v1: i32,
        v2: i32,
    }
    assert_eq!(vars.get::<i32>("a").unwrap(), 10);
    assert_eq!(vars.get::<String>("b").unwrap(), "abc");
    assert_eq!(vars.get::<Vec<i32>>("c").unwrap(), vec![1, 2]);

    let obj = vars.get::<Obj>("d").unwrap();
    assert_eq!(obj.v1, 1);
    assert_eq!(obj.v2, 2);
}

#[test]
fn model_workflow_from_yml_str_exposes() {
    let text = r#"
    exposes:
        - name: a
          value: 10
        - name: b
          value: abc
        - name: c
          value: 
            - 1
            - 2
        - name: d
          value: 
            v1: 1
            v2: 2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let exposes = m.exposes();

    #[derive(Serialize, Deserialize, Clone)]
    struct Obj {
        v1: i32,
        v2: i32,
    }
    assert_eq!(exposes.get::<i32>("a").unwrap(), 10);
    assert_eq!(exposes.get::<String>("b").unwrap(), "abc");
    assert_eq!(exposes.get::<Vec<i32>>("c").unwrap(), vec![1, 2]);

    let obj = exposes.get::<Obj>("d").unwrap();
    assert_eq!(obj.v1, 1);
    assert_eq!(obj.v2, 2);
}

#[test]
fn model_workflow_from_yml_inputs_simple() {
    let text = r#"
    inputs:
        name: input
        type: string
        title: Input Title
        desc: Input Description
        required: true
        value: default value
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let inputs = m.inputs.simple().unwrap();
    assert_eq!(inputs.r#type, VariantTypes::String);
    assert_eq!(inputs.name, "input");
    assert_eq!(inputs.title, "Input Title");
    assert_eq!(inputs.desc, "Input Description");
    assert!(inputs.required);
    assert_eq!(inputs.value, "default value");
}

#[test]
fn model_workflow_exposes_untyped_type_follows_value() {
    // name-only exposes are validated by the runtime type of the exported
    // value, not by a default `string` type
    let text = r#"
    exposes:
        - name: result
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let exposes = m.exposes.first().unwrap();
    assert_eq!(exposes.name, "result");
    assert_eq!(exposes.r#type, VariantTypes::String);
    assert!(!exposes.typed);

    let schema = ActSchema::Multiple(m.exposes.clone());
    assert!(
        schema.validate(&json!({ "result": 10 })).is_ok(),
        "a number export must not fail as if it were a string"
    );
    assert!(schema.validate(&json!({ "result": true })).is_ok());
}

#[test]
fn model_workflow_exposes_literal_value_infers_type() {
    // a literal value without `type` declares the matching concrete type
    let text = r#"
    exposes:
        - name: result
          value: 10
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let exposes = m.exposes.first().unwrap();
    assert_eq!(exposes.r#type, VariantTypes::Number);
    assert!(exposes.typed);
}

#[test]
fn model_workflow_exposes_explicit_type_stays_strict() {
    let text = r#"
    exposes:
        - name: result
          type: string
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let schema = ActSchema::Multiple(m.exposes.clone());
    assert!(schema.validate(&json!({ "result": "ok" })).is_ok());
    assert!(schema.validate(&json!({ "result": 10 })).is_err());
}

#[test]
fn model_workflow_exposes_untyped_roundtrip_yml() {
    // serializing a model must not write back an implicit `type: string`
    let text = r#"
    exposes:
        - name: result
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let yml = m.to_yml().unwrap();
    assert!(!yml.contains("type"));
    let m2 = Workflow::from_yml(&yml).unwrap();
    assert!(!m2.exposes.first().unwrap().typed);
}

#[test]
fn model_workflow_from_yml_inputs_multiple() {
    let text = r#"
    inputs:
        - name: input1
          type: string
          title: Input 1 Title
          desc: Input 1 Description
          required: true
          value: default value 1
        - name: input2
          type: number  
          title: Input 2 Title    
          desc: Input 2 Description
          required: false
          value: 42
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let inputs = m.inputs.multiple().unwrap();
    assert_eq!(inputs.len(), 2);

    let input = &inputs[0];
    assert_eq!(input.r#type, VariantTypes::String);
    assert_eq!(input.name, "input1");
    assert_eq!(input.title, "Input 1 Title");
    assert_eq!(input.desc, "Input 1 Description");
    assert!(input.required);
    assert_eq!(input.value, "default value 1");

    let input = &inputs[1];
    assert_eq!(input.r#type, VariantTypes::Number);
    assert_eq!(input.name, "input2");
    assert_eq!(input.title, "Input 2 Title");
    assert_eq!(input.desc, "Input 2 Description");
    assert!(!input.required);
    assert_eq!(input.value, 42);
}

#[test]
fn model_workflow_from_yml_output_simple() {
    let text = r#"
    exposes:
        - name: output
          type: string
          title: Output Title
          desc: Output Description
          required: true
          value: default value
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let outputs = m.exposes.first().unwrap();
    assert_eq!(outputs.r#type, VariantTypes::String);
    assert_eq!(outputs.name, "output");
    assert_eq!(outputs.title, "Output Title");
    assert_eq!(outputs.desc, "Output Description");
    assert!(outputs.required);
    assert_eq!(outputs.value, "default value");
}

#[test]
fn model_workflow_from_yml_outputs_multiple() {
    let text = r#"
    exposes:
        - name: output1
          type: string
          title: Output 1 Title
          desc: Output 1 Description
          required: true
          value: default value 1
        - name: output2
          type: number  
          title: Output 2 Title    
          desc: Output 2 Description
          required: false
          value: 42
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let outputs = &m.exposes;
    assert_eq!(outputs.len(), 2);

    let output = &outputs[0];
    assert_eq!(output.r#type, VariantTypes::String);
    assert_eq!(output.name, "output1");
    assert_eq!(output.title, "Output 1 Title");
    assert_eq!(output.desc, "Output 1 Description");
    assert!(output.required);
    assert_eq!(output.value, "default value 1");

    let output = &outputs[1];
    assert_eq!(output.r#type, VariantTypes::Number);
    assert_eq!(output.name, "output2");
    assert_eq!(output.title, "Output 2 Title");
    assert_eq!(output.desc, "Output 2 Description");
    assert!(!output.required);
    assert_eq!(output.value, 42);
}

#[test]
fn model_workflow_from_json_str_name_id() {
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
    assert_eq!(m.vars().get::<i32>("v1").unwrap(), 5);
}

#[test]
fn model_workflow_set_env() {
    let mut m = Workflow::new();
    let vars = vec![Variant::create("v1", 5)];
    m.set_env(&vars);
    assert_eq!(m.env().get_value("v1"), Some(&json!(5)));
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
    let m = Workflow::new().with_option("tag", "tag1");
    assert_eq!(m.options.get::<String>("tag").unwrap(), "tag1");
}

#[test]
fn model_workflow_desc() {
    let m = Workflow::new().with_desc("desc1");
    assert_eq!(m.desc, "desc1");
}

#[test]
fn model_workflow_default_ver() {
    let m = Workflow::new();
    assert_eq!(m.ver, "0.1.0");
}

#[test]
fn model_workflow_set_ver() {
    let m = Workflow::new().with_ver("0.2.0");
    assert_eq!(m.ver, "0.2.0");
}

#[test]
fn model_workflow_rn() {
    let m = Workflow::new().with_option("rn", "a:b:c");
    assert_eq!(m.options.get::<String>("rn").unwrap(), "a:b:c");
}

#[test]
fn model_workflow_options() {
    let mut m = Workflow::new();
    assert!(m.options.is_empty());

    m = m.with_option("max_limit", 5);
    assert_eq!(m.options.get::<i32>("max_limit").unwrap(), 5);
}

#[test]
fn model_workflow_inputs_schema() {
    let schema = ActSchema::Simple(Variant::new().name("input").r#type(VariantTypes::String));
    let m = Workflow::new().with_inputs(schema.clone());

    let var = m.inputs.simple().unwrap();
    assert_eq!(var.name, "input");
    assert_eq!(var.r#type, VariantTypes::String);
}

#[test]
fn model_workflow_outputs_schema() {
    let mut m = Workflow::new();
    m.exposes
        .push(Variant::new().name("data").r#type(VariantTypes::String));
    let var = &m.exposes[0];
    assert_eq!(var.name, "data");
    assert_eq!(var.r#type, VariantTypes::String);
}

#[test]
fn model_workflow_on_event() {
    let workflow = Workflow::new()
        .with_id("my-event-model")
        .with_trigger(|t| {
            t.with_id("event1")
                .with_kind("manual")
                .with_params_vars(|vars| vars.with("test", 10))
        })
        .with_trigger(|t| {
            t.with_id("event2")
                .with_kind("schedule")
                .with_schedule("* * * * * *")
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

#[test]
fn model_workflow_with_expose() {
    let m = Workflow::new().with_expose(Variant::create("v1", 0));
    assert!(m.exposes().contains_key("v1"));
}

#[test]
fn model_workflow_model_base_id() {
    use crate::ModelBase;
    let m = Workflow::new().with_id("wf-id");
    assert_eq!(m.id(), "wf-id");
}

#[test]
fn model_workflow_from_yml_and_json_errors() {
    let err = Workflow::from_yml("steps: not-a-list").unwrap_err();
    assert!(matches!(err, crate::ActError::Model(_)), "{err}");

    let err = Workflow::from_json("{\"steps\": 42}").unwrap_err();
    assert!(matches!(err, crate::ActError::Model(_)), "{err}");
}

#[test]
fn model_workflow_step_lookup() {
    let m = Workflow::new()
        .with_step(|step| step.with_id("s1"))
        .with_step(|step| step.with_id("s2"));
    assert_eq!(m.step("s2").unwrap().id, "s2");
    assert!(m.step("missing").is_none());
}

#[test]
fn model_workflow_with_env_updates_existing() {
    let m = Workflow::new()
        .with_env("a", 1)
        .with_env("b", 2)
        .with_env("a", 10);
    assert_eq!(m.env.len(), 2);
    assert_eq!(m.env().get::<i32>("a").unwrap(), 10);

    let mut m = Workflow::new();
    m.set_env(&[Variant::create("a", 1), Variant::create("b", 2)]);
    m.set_env(&[Variant::create("a", 10)]);
    assert_eq!(m.env.len(), 2);
    assert_eq!(m.env().get::<i32>("a").unwrap(), 10);
}

#[test]
fn model_workflow_rn_accepts_colon_segments() {
    let m = Workflow::new().with_id("m1").with_rn("orders:eu");
    assert!(m.valid().is_ok());
}

#[test]
fn model_workflow_rn_rejects_malformed_names() {
    for rn in ["a b", "a*", "a?", "a[b]", "a{b}", "a::b", ":a", "a:"] {
        let m = Workflow::new().with_id("m1").with_rn(rn);
        let err = m.valid().unwrap_err();
        assert!(err.to_string().contains("invalid rn"), "{rn}: {err}");
    }
}

#[test]
fn model_workflow_valid_rejects_duplicate_trigger_ids() {
    let m = Workflow::new()
        .with_id("m1")
        .with_trigger(|t| t.with_id("t1").with_kind("manual"))
        .with_trigger(|t| t.with_id("t1").with_kind("manual"));
    let err = m.valid().unwrap_err();
    assert!(err.to_string().contains("dup event id"), "{err}");
}

#[test]
fn model_workflow_valid_rejects_bad_triggers() {
    let m = Workflow::new()
        .with_id("m1")
        .with_trigger(|t| t.with_id("t1").with_kind("schedule"));
    assert!(m.valid().is_err());

    let m = Workflow::new()
        .with_id("m1")
        .with_trigger(|t| t.with_id("t1").with_kind("custom"));
    assert!(m.valid().is_ok());
}

#[test]
fn model_workflow_valid_rejects_while_with_next() {
    let m = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("s1").with_while("${{ a }}").with_next("s1"));
    let err = m.valid().unwrap_err();
    assert!(err.to_string().contains("cannot combine 'while'"), "{err}");
}

#[test]
fn model_workflow_valid_rejects_forward_next() {
    let m = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("s1").with_next("s2"));
    let err = m.valid().unwrap_err();
    assert!(err.to_string().contains("found next node error"), "{err}");
}

#[test]
fn model_workflow_valid_accepts_declared_next() {
    let m = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("s1"))
        .with_step(|step| step.with_id("s2").with_next("s1"));
    assert!(m.valid().is_ok());
}

#[test]
fn model_workflow_valid_rejects_duplicate_branch_and_nested_ids() {
    let m = Workflow::new()
        .with_id("m1")
        .with_step(|step| step.with_id("s1").with_branch(|b| b.with_id("b1")))
        .with_step(|step| step.with_id("s2").with_branch(|b| b.with_id("b1")));
    assert!(m.valid().is_err());

    let m = Workflow::new().with_id("m1").with_step(|step| {
        step.with_id("s1")
            .with_branch(|b| b.with_id("b1").with_step(|inner| inner.with_id("s1")))
    });
    assert!(m.valid().is_err());

    let m = Workflow::new().with_id("m1").with_step(|step| {
        step.with_id("s1")
            .with_timeout(|inner| inner.with_id("t1"))
            .with_timeout(|inner| inner.with_id("t1"))
    });
    assert!(m.valid().is_err());
}

#[test]
fn model_workflow_tree_output_covers_branches_links_and_catches() {
    let m = Workflow::new()
        .with_id("m1")
        .with_step(|step| {
            step.with_id("s1")
                .with_branch(|b| b.with_id("b1").with_step(|inner| inner.with_id("sb1")))
                .with_catch(|c| c.with_id("c1"))
                .with_timeout(|t| t.with_id("to1"))
        })
        .with_step(|step| step.with_id("s2").with_next("s1"));
    let output = m.tree_output();
    assert!(output.contains("id:m1"), "{output}");
    assert!(output.contains("id:b1"), "{output}");
    assert!(output.contains("id:sb1"), "{output}");
    assert!(output.contains("id:c1"), "{output}");
    assert!(output.contains("id:to1"), "{output}");
}
