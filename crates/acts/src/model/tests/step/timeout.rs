use crate::{
    Act, Step, Workflow,
    model::act::{TimeoutLimit, TimeoutUnit},
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
    assert_eq!(step.timeouts.len(), 0);

    step = step
        .with_timeout(Act::msg(|msg| {
            msg.with_key("msg1").with_if(r#"$cost_in('1h')"#)
        }))
        .with_timeout(Act::msg(|msg| {
            msg.with_key("msg2").with_if(r#"$cost_in('2d')"#)
        }));

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
