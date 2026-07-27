use crate::{Act, Step, Variant, Vars, Workflow, utils::test::USES_MSG};
use serde_json::json;

#[test]
fn model_act_parse_nest() {
    let text = r#"
    uses: acts.transform.parallel
    in: "[\"a\", \"b\"]"
    acts:
        - uses: acts.core.msg
          key: msg1
        - uses: acts.core.set
          inputs:
            a: 10
        - act: acts.transform.parallel
          in: "[\"a\", \"b\"]"
          acts:
            - uses: acts.core.msg
              inputs:
                key: msg2
            - uses: acts.core.msg
              if: $("a") > 0
              key: msg3

    "#;
    assert!(serde_yaml::from_str::<Act>(text).is_ok());
}

#[test]
fn model_act_to_json() {
    let text = r#"
    - uses: acts.transform.parallel
      in: "[\"a\", \"b\"]"
      acts:
          - uses: acts.core.msg
            params:
                key: msg1
          - uses: acts.transform.parallel
            params:
                in: "[\"a\", \"b\"]"
                acts:
                    - uses: acts.core.msg
                      params:
                        key: msg2
    - uses: acts.core.msg
      params:
        key: msg2
    "#;

    let stms: Vec<Act> = serde_yaml::from_str(text).unwrap();
    let ret = serde_json::to_string(&stms);
    assert!(ret.is_ok());
}

#[test]
fn model_act_set_name() {
    let act = Act::new().with_name("name1");
    assert_eq!(act.name, "name1");
}

#[test]
fn model_act_set_desc() {
    let act = Act::new().with_desc("desc1");
    assert_eq!(act.desc, "desc1");
}

#[test]
fn model_act_set_uses() {
    let act = Act::new().with_uses("uses1");
    assert_eq!(act.uses, "uses1");
}

#[test]
fn model_act_set_id() {
    let act = Act::new().with_id("act1");
    assert_eq!(act.id, "act1");
}

#[test]
fn model_act_set_params() {
    let params = json!({ "a": 1, "b": "value1" });
    let act = Act::new().with_params_data(params.clone());
    assert_eq!(act.params, params);
}

#[test]
fn model_act_set_var() {
    let act = Act::new().with_var("var1", 1);
    assert_eq!(act.vars().get::<i32>("var1").unwrap(), 1);
}

#[test]
fn model_act_set_output() {
    let act = Act::new().with_expose(Variant::create("var1", 1));
    assert_eq!(act.exposes().get::<i32>("var1").unwrap(), 1);
}

#[test]
fn model_act_set_if() {
    let act = Act::new().with_if("{{ var1 > 1 }}");
    assert_eq!(act.r#if.unwrap(), "{{ var1 > 1 }}");
}

#[test]
fn model_act_set_tag() {
    let b = Act::new().with_option("tag", "tag1");
    assert_eq!(b.options.get::<String>("tag").unwrap(), "tag1");
}

#[test]
fn model_act_options() {
    let mut b = Act::new();
    assert!(b.options.is_empty());

    b = b.with_option("max_limit", 5);
    assert_eq!(b.options.get::<i32>("max_limit").unwrap(), 5);
}

#[test]
fn model_act_set_params_key() {
    let b = Act::new().with_params_vars(|v| v.with("key", "key1"));
    assert_eq!(b.params.get("key").unwrap(), "key1");
}

#[test]
fn model_act_set_metadata() {
    let act = Act::new()
        .with_metadata("r1", 1)
        .with_metadata("r2", "abc")
        .with_metadata("r3", json!(["a", "b"]))
        .with_metadata("r4", true);

    assert_eq!(act.metadata.get::<i32>("r1").unwrap(), 1);
    assert_eq!(act.metadata.get::<String>("r2").unwrap(), "abc");
    assert_eq!(
        act.metadata.get::<Vec<String>>("r3").unwrap(),
        vec!["a", "b"]
    );
    assert!(act.metadata.get::<bool>("r4").unwrap());
}

#[test]
fn model_act_yml_vars() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          vars:
            - name: p1
              value: 5

    "#;
    let m = Workflow::from_yml(text).unwrap();

    let step = m.steps.first().unwrap();
    assert_eq!(step.vars.len(), 1);
    assert_eq!(step.vars().get_value("p1"), Some(&json!(5)));
}

#[test]
fn model_act_yml_expose() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          exposes:
            - name: p1
    "#;

    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let exposes = step.exposes();
    assert_eq!(exposes.len(), 1);
    assert_eq!(exposes.get_value("p1"), Some(&json!(null)));
}

#[test]
fn model_act_with_expose() {
    let act = Act::new().with_expose(Variant::create("v1", 0));
    assert!(act.exposes().contains_key("v1"));
}

#[test]
fn model_act_timeout() {
    let mut step = Step::new();
    assert_eq!(step.timeouts.len(), 0);

    step = step
        .with_timeout(|step| {
            step.with_if(r#"$cost_in('1h')"#)
                .with_uses(USES_MSG, Vars::new())
        })
        .with_timeout(|step| {
            step.with_if(r#"$cost_in('2d')"#)
                .with_uses(USES_MSG, Vars::new())
        });

    assert_eq!(step.timeouts.len(), 2);

    let timeout = step.timeouts.first().unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('1h')");

    let timeout = step.timeouts.get(1).unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('2d')");
}

#[test]
fn model_act_yml_timeout() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          timeouts:
            - uses: acts.core.irq
              if: $cost_in('2d')
            - uses: acts.core.irq
              if: $cost_in('3m')
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.timeouts.len(), 2);

    let timeout = step.timeouts.first().unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('2d')");

    let timeout = step.timeouts.get(1).unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('3m')");
}

#[test]
fn model_act_catch() {
    let mut step = Step::new();
    assert_eq!(step.catches.len(), 0);

    step = step
        .with_catch(|step| step.with_if(r#"$ecode() == "err1""#))
        .with_catch(|step| step);
    assert_eq!(step.catches.len(), 2);
    assert_eq!(
        step.catches.first().unwrap().r#if,
        Some(r#"$ecode() == "err1""#.to_string())
    );
    assert_eq!(step.catches.get(1).unwrap().r#if, None);
}

#[test]
fn model_act_yml_catches_err() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          catches:
            - uses: acts.core.irq
              if: $ecode() == 'err1'
            - uses: acts.core.irq
              if: $ecode() == 'err2'

    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 2);

    let catch = step.catches.get(1).unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), "$ecode() == 'err2'");
}

#[test]
fn model_act_yml_catches_all() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          catches:
            - uses: acts.core.irq
              if: $ecode() == 'err1'
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 1);

    let catch = step.catches.first().unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), "$ecode() == 'err1'");
}
