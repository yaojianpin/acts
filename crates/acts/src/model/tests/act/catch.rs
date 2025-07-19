use crate::{Act, Workflow};

#[test]
fn model_act_catch() {
    let mut act = Act::new();
    assert_eq!(act.catches.len(), 0);

    act = act
        .with_catch(Act::new().with_if(r#"$ecode() == "err1""#))
        .with_catch(Act::new());
    assert_eq!(act.catches.len(), 2);
    assert_eq!(
        act.catches.first().unwrap().r#if,
        Some(r#"$ecode() == "err1""#.to_string())
    );
    assert_eq!(act.catches.get(1).unwrap().r#if, None);
}

#[test]
fn model_act_yml_catches_err() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - uses: acts.core.irq
              catches:
                - uses: acts.core.irq
                  if: $ecode() == 'err1'
                - uses: acts.core.irq
                  if: $ecode() == 'err2'

    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.catches.len(), 2);

    let catch = act.catches.get(1).unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), "$ecode() == 'err2'");
}

#[test]
fn model_act_yml_catches_all() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - uses: acts.core.irq
              catches:
                - uses: acts.core.irq
                  key: act2
                  if: $ecode() == 'err1'
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.catches.len(), 1);

    let catch = act.catches.first().unwrap();
    assert_eq!(catch.r#if.as_ref().unwrap(), "$ecode() == 'err1'");
}
