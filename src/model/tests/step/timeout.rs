use crate::{
    model::step::{TimeoutLimit, TimeoutUnit},
    Act, Step, StmtBuild, Workflow,
};

#[test]
fn model_timeout_parse_seconds() {
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
fn model_timeout_parse_minutes() {
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
fn model_timeout_parse_hours() {
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
fn model_timeout_parse_days() {
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
fn model_timeout_parse_error() {
    let timeout = TimeoutLimit::parse("");

    assert_eq!(timeout.is_err(), true);

    let timeout = TimeoutLimit::parse("100x");
    assert_eq!(timeout.is_err(), true);

    let timeout = TimeoutLimit::parse("xxd");
    assert_eq!(timeout.is_err(), true);

    let timeout = TimeoutLimit::parse("100");
    assert_eq!(timeout.is_err(), true);
}

#[test]
fn model_timeout_to_string() {
    let timeout = TimeoutLimit::parse("2d").unwrap();
    assert_eq!(timeout.to_string(), "2d");
}

#[test]
fn model_timeout_ser() {
    let timeout = TimeoutLimit {
        value: 2,
        unit: TimeoutUnit::Day,
    };
    assert_eq!(serde_json::ser::to_string(&timeout).unwrap(), r#""2d""#);
}

#[test]
fn model_timeout_deser() {
    let timeout: TimeoutLimit = serde_json::de::from_str(r#""2d""#).unwrap();
    assert_eq!(timeout.value, 2);
    assert_eq!(timeout.unit, TimeoutUnit::Day);
    assert_eq!(timeout.as_secs(), 2 * 60 * 60 * 24);
}

#[test]
fn model_step_timeout() {
    let mut step = Step::new();
    assert_eq!(step.timeout.len(), 0);

    step = step
        .with_timeout(|t| {
            t.with_on("1h")
                .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg1"))))
        })
        .with_timeout(|t| {
            t.with_on("2d")
                .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_id("msg2"))))
        });

    assert_eq!(step.timeout.len(), 2);
    assert_eq!(step.timeout.get(0).unwrap().on.value, 1);
    assert_eq!(step.timeout.get(0).unwrap().on.unit, TimeoutUnit::Hour);
    assert_eq!(step.timeout.get(1).unwrap().on.value, 2);
    assert_eq!(step.timeout.get(1).unwrap().on.unit, TimeoutUnit::Day);
}

#[test]
fn model_step_yml_timeout() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          timeout:
            - on: 2d
            - on: 3m
              then:
                - !req
                  id: act2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.get(0).unwrap();
    assert_eq!(step.timeout.len(), 2);

    let timeout = step.timeout.get(0).unwrap();
    assert_eq!(timeout.on.value, 2);
    assert_eq!(timeout.on.as_secs(), 2 * 24 * 60 * 60);

    let timeout = step.timeout.get(1).unwrap();
    assert_eq!(timeout.on.value, 3);
    assert_eq!(timeout.on.as_secs(), 3 * 60);
    assert_eq!(timeout.then.len(), 1);
}
