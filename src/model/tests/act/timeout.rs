use crate::{model::act::TimeoutUnit, Act, StmtBuild, Workflow};

#[test]
fn model_act_timeout() {
    let mut act = Act::new();
    assert_eq!(act.timeout.len(), 0);

    act = act
        .with_timeout(|t| {
            t.with_on("1h")
                .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg1"))))
        })
        .with_timeout(|t| {
            t.with_on("2d")
                .with_then(|stmts| stmts.add(Act::msg(|msg| msg.with_key("msg2"))))
        });

    assert_eq!(act.timeout.len(), 2);
    assert_eq!(act.timeout.first().unwrap().on.value, 1);
    assert_eq!(act.timeout.first().unwrap().on.unit, TimeoutUnit::Hour);
    assert_eq!(act.timeout.get(1).unwrap().on.value, 2);
    assert_eq!(act.timeout.get(1).unwrap().on.unit, TimeoutUnit::Day);
}

#[test]
fn model_act_yml_timeout() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - act: irq
              timeout:
                - on: 2d
                - on: 3m
                  then:
                    - act: irq
                      key: act2
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.timeout.len(), 2);

    let timeout = act.timeout.first().unwrap();
    assert_eq!(timeout.on.value, 2);
    assert_eq!(timeout.on.as_secs(), 2 * 24 * 60 * 60);

    let timeout = act.timeout.get(1).unwrap();
    assert_eq!(timeout.on.value, 3);
    assert_eq!(timeout.on.as_secs(), 3 * 60);
    assert_eq!(timeout.then.len(), 1);
}
