use crate::{Act, TimeoutLimit, Workflow, model::act::TimeoutUnit};

#[test]
fn model_act_timeout() {
    let mut act = Act::new();
    assert_eq!(act.timeout.len(), 0);

    act = act
        .with_timeout(|t| {
            t.with_on("1h")
                .with_step(|step| step.with_act(Act::msg(|msg| msg.with_key("msg1"))))
        })
        .with_timeout(|t| {
            t.with_on("2d")
                .with_step(|step| step.with_act(Act::msg(|msg| msg.with_key("msg2"))))
        });

    assert_eq!(act.timeout.len(), 2);

    let timeout1 = TimeoutLimit::parse(&act.timeout.first().unwrap().on).unwrap();
    assert_eq!(timeout1.value, 1);
    assert_eq!(timeout1.unit, TimeoutUnit::Hour);

    let timeout2 = TimeoutLimit::parse(&act.timeout.get(1).unwrap().on).unwrap();
    assert_eq!(timeout2.value, 2);
    assert_eq!(timeout2.unit, TimeoutUnit::Day);
}

#[test]
fn model_act_yml_timeout() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - uses: acts.core.irq
              timeout:
                - on: 2d
                - on: 3m
                  steps:
                    - id: step1
                      acts:
                        - uses: acts.core.irq
                          key: act2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.timeout.len(), 2);

    let timeout = act.timeout.first().unwrap();
    let timeout_limit = TimeoutLimit::parse(&timeout.on).unwrap();
    assert_eq!(timeout_limit.value, 2);
    assert_eq!(timeout_limit.as_secs(), 2 * 24 * 60 * 60);

    let timeout = act.timeout.get(1).unwrap();
    let timeout_limit = TimeoutLimit::parse(&timeout.on).unwrap();
    assert_eq!(timeout_limit.value, 3);
    assert_eq!(timeout_limit.as_secs(), 3 * 60);
    assert_eq!(timeout.steps.len(), 1);
}
