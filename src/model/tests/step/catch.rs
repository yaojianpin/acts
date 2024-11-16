use crate::{Step, Workflow};

#[test]
fn model_step_catch() {
    let mut step = Step::new();
    assert_eq!(step.catches.len(), 0);

    step = step.with_catch(|c| c.with_on("err1")).with_catch(|c| c);
    assert_eq!(step.catches.len(), 2);

    assert_eq!(step.catches.first().unwrap().on.as_ref().unwrap(), "err1");
    assert_eq!(step.catches.get(1).unwrap().on, None);
}

#[test]
fn model_step_yml_catches_err() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          catches:
            - on: err1
            - on: err2
              then:
                - act: irq
                  key: act2

    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 2);

    let catch = step.catches.get(1).unwrap();
    assert_eq!(catch.on.as_ref().unwrap(), "err2");
    assert_eq!(catch.then.len(), 1);
}

#[test]
fn model_step_yml_catches_all() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          catches:
            - on: err1
            - then:
                - act: irq
                  key: act2
                - act: msg
                  key: msg1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.catches.len(), 2);

    let catch = step.catches.first().unwrap();
    assert_eq!(catch.on.as_ref().unwrap(), "err1");
    assert_eq!(catch.then.len(), 0);

    let catch = step.catches.get(1).unwrap();
    assert_eq!(catch.on, None);
    assert_eq!(catch.then.len(), 2);
}
