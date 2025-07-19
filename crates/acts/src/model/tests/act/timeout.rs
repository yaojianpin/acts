use crate::{Act, Workflow};

#[test]
fn model_act_timeout() {
    let mut act = Act::new();
    assert_eq!(act.timeouts.len(), 0);

    act = act
        .with_timeout(Act::msg(|msg| {
            msg.with_key("msg1").with_if(r#"$cost_in('1h')"#)
        }))
        .with_timeout(Act::msg(|msg| {
            msg.with_key("msg2").with_if(r#"$cost_in('2d')"#)
        }));

    assert_eq!(act.timeouts.len(), 2);

    let timeout = act.timeouts.first().unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('1h')");

    let timeout = act.timeouts.get(1).unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('2d')");
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
              timeouts:
                - uses: acts.core.irq
                  if: $cost_in('2d')
                - uses: acts.core.irq
                  if: $cost_in('3m')
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.timeouts.len(), 2);

    let timeout = act.timeouts.first().unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('2d')");

    let timeout = act.timeouts.get(1).unwrap();
    assert_eq!(timeout.r#if.as_ref().unwrap(), "$cost_in('3m')");
}
