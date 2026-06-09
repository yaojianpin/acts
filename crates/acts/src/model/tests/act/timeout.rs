use crate::{Step, Vars, Workflow, utils::test::USES_MSG};

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
