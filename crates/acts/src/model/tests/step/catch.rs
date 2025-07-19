use crate::{Act, Step, Workflow};

#[test]
fn model_step_catch() {
    let mut step = Step::new();
    assert_eq!(step.catches.len(), 0);

    step = step
        .with_catch(Act::default().with_if(r#"$ecode() == "err1""#))
        .with_catch(Act::default());
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
