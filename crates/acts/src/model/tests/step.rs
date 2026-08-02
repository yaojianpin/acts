use crate::{
    Step, TimeoutLimit, Variant, Vars, Workflow,
    model::act::TimeoutUnit,
    utils::test::{USES_IRQ, USES_MSG},
};
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
          exposes:
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
    let step = Step::new().with_expose(Variant::create("p1", json!(5)));

    let exposes = step.exposes();
    assert!(exposes.get_value("p1").is_some());
}

#[test]
fn model_step_tag() {
    let step = Step::new().with_option("tag", "tag1");
    assert_eq!(step.options.get::<String>("tag").unwrap(), "tag1");
}

#[test]
fn model_step_next() {
    let mut step = Step::new();
    assert!(step.next.is_none());

    step = step.with_next("step1");
    assert_eq!(step.next.unwrap(), "step1");
}

#[test]
fn model_step_if() {
    let mut step = Step::new();
    assert!(step.r#if.is_none());

    step = step.with_if("true");
    assert_eq!(step.r#if.unwrap(), "true");
}

#[test]
fn model_step_rn() {
    let mut step = Step::new();
    assert!(step.options.get::<String>("rn").is_none());

    step = step.with_option("rn", "a:b:c");
    assert_eq!(step.options.get::<String>("rn").unwrap(), "a:b:c");
}

#[test]
fn model_step_options() {
    let mut step = Step::new();
    assert!(step.options.is_empty());

    step = step.with_option("max_limit", 5);
    assert_eq!(step.options.get::<i32>("max_limit").unwrap(), 5);
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
fn model_step_uses() {
    let step = Step::new().with_uses(USES_IRQ, Vars::new().with("key", "act1"));
    assert_eq!(step.uses, Some(USES_IRQ.to_string()));
    assert_eq!(step.params.get("key").unwrap(), "act1");
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

#[test]
fn model_step_with_expose() {
    let step = Step::new().with_expose(Variant::create("v1", 0));
    assert!(step.exposes().contains_key("v1"));
}

#[test]
fn model_step_yml_uses() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          params:
            a: 5
        - id: step2
          uses: acts.core.msg
          params:
            b: ${{ a }}
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.steps.len(), 2);
}

#[test]
fn model_step_step_catch() {
    let mut step = Step::new();
    assert_eq!(step.catches.len(), 0);

    step = step
        .with_catch(|step| {
            step.with_if(r#"$ecode() == "err1""#)
                .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
        })
        .with_catch(|step| step);
    assert_eq!(step.catches.len(), 2);

    assert_eq!(
        step.catches.first().unwrap().r#if,
        Some(r#"$ecode() == "err1""#.to_string())
    );
    assert_eq!(step.catches.get(1).unwrap().r#if, None);
}

#[test]
fn model_step_yml_catches_err() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          catches:
            - key: act1
              if: $ecode() == "err1"
            - act: acts.core.irq
              key: act2
              if: $ecode() == "err2"
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 2);

    let catch = step.catches.get(1).unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), r#"$ecode() == "err2""#);
}

#[test]
fn model_step_yml_catches_all() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          catches:
            - key: act1
              if: $ecode() == "err1"
            - key: act2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 2);

    let catch = step.catches.first().unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), r#"$ecode() == "err1""#);

    let catch = step.catches.get(1).unwrap();
    assert_eq!(catch.r#if, None);
}

#[test]
fn model_step_timeout_parse_seconds() {
    let timeout = TimeoutLimit::parse("1s").unwrap();

    assert_eq!(timeout.value, 1);
    assert_eq!(timeout.unit, TimeoutUnit::Second);
    assert_eq!(timeout.as_secs(), 1);

    let timeout = TimeoutLimit::parse("100s").unwrap();

    assert_eq!(timeout.value, 100);
    assert_eq!(timeout.unit, TimeoutUnit::Second);
    assert_eq!(timeout.as_secs(), 100);
}

#[test]
fn model_step_timeout_parse_minutes() {
    let timeout = TimeoutLimit::parse("1m").unwrap();

    assert_eq!(timeout.value, 1);
    assert_eq!(timeout.unit, TimeoutUnit::Minute);
    assert_eq!(timeout.as_secs(), 60);

    let timeout = TimeoutLimit::parse("100m").unwrap();

    assert_eq!(timeout.value, 100);
    assert_eq!(timeout.unit, TimeoutUnit::Minute);
    assert_eq!(timeout.as_secs(), 100 * 60);
}

#[test]
fn model_step_timeout_parse_hours() {
    let timeout = TimeoutLimit::parse("1h").unwrap();

    assert_eq!(timeout.value, 1);
    assert_eq!(timeout.unit, TimeoutUnit::Hour);
    assert_eq!(timeout.as_secs(), 60 * 60);

    let timeout = TimeoutLimit::parse("100h").unwrap();

    assert_eq!(timeout.value, 100);
    assert_eq!(timeout.unit, TimeoutUnit::Hour);
    assert_eq!(timeout.as_secs(), 100 * 60 * 60);
}

#[test]
fn model_step_timeout_parse_days() {
    let timeout = TimeoutLimit::parse("1d").unwrap();

    assert_eq!(timeout.value, 1);
    assert_eq!(timeout.unit, TimeoutUnit::Day);
    assert_eq!(timeout.as_secs(), 60 * 60 * 24);

    let timeout = TimeoutLimit::parse("100d").unwrap();

    assert_eq!(timeout.value, 100);
    assert_eq!(timeout.unit, TimeoutUnit::Day);
    assert_eq!(timeout.as_secs(), 100 * 60 * 60 * 24);
}

#[test]
fn model_step_timeout_parse_error() {
    let timeout = TimeoutLimit::parse("");

    assert!(timeout.is_err());

    let timeout = TimeoutLimit::parse("100x");
    assert!(timeout.is_err());

    let timeout = TimeoutLimit::parse("xxd");
    assert!(timeout.is_err());

    let timeout = TimeoutLimit::parse("100");
    assert!(timeout.is_err());
}

#[test]
fn model_timeout_to_string() {
    let timeout = TimeoutLimit::parse("2d").unwrap();
    assert_eq!(timeout.to_string(), "2d");
}

#[test]
fn model_step_timeout_ser() {
    let timeout = TimeoutLimit {
        value: 2,
        unit: TimeoutUnit::Day,
    };
    assert_eq!(serde_json::ser::to_string(&timeout).unwrap(), r#""2d""#);
}

#[test]
fn model_step_timeout_deser() {
    let timeout: TimeoutLimit = serde_json::de::from_str(r#""2d""#).unwrap();
    assert_eq!(timeout.value, 2);
    assert_eq!(timeout.unit, TimeoutUnit::Day);
    assert_eq!(timeout.as_secs(), 2 * 60 * 60 * 24);
}

#[test]
fn model_step_timeout() {
    let mut step = Step::new();
    assert_eq!(step.timeouts.len(), 0);

    step = step
        .with_timeout(|step| {
            step.with_id("timout1")
                .with_if(r#"$cost_in('1h')"#)
                .with_uses(USES_MSG, Vars::new().with("key", "msg1"))
        })
        .with_timeout(|step| {
            step.with_id("timout2")
                .with_if(r#"$cost_in('2d')"#)
                .with_uses(USES_MSG, Vars::new().with("key", "msg2"))
        });

    assert_eq!(step.timeouts.len(), 2);
    assert_eq!(
        step.timeouts
            .first()
            .as_ref()
            .unwrap()
            .r#if
            .as_ref()
            .unwrap(),
        "$cost_in('1h')"
    );
    assert_eq!(
        step.timeouts
            .get(1)
            .as_ref()
            .unwrap()
            .r#if
            .as_ref()
            .unwrap(),
        "$cost_in('2d')"
    );
}

#[test]
fn model_step_yml_timeout() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          timeouts:
            - uses: acts.core.irq
              key: act2
              if: $cost_in('2d')
            - uses: acts.core.irq
              key: act3
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
