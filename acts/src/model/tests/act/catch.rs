use crate::{Act, Workflow};

#[test]
fn model_act_catch() {
    let mut act = Act::new();
    assert_eq!(act.catches.len(), 0);

    act = act.with_catch(|c| c.with_on("err1")).with_catch(|c| c);
    assert_eq!(act.catches.len(), 2);

    assert_eq!(act.catches.first().unwrap().on.as_ref().unwrap(), "err1");
    assert_eq!(act.catches.get(1).unwrap().on, None);
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
                - on: err1
                - on: err2
                  steps:
                    - id: step2
                      acts:
                        - uses: acts.core.irq
                          key: act2

    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.catches.len(), 2);

    let catch = act.catches.get(1).unwrap();
    assert_eq!(catch.on.as_ref().unwrap(), "err2");
    assert_eq!(catch.steps.len(), 1);
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
                - on: err1
                - steps:
                    - id: step2
                      acts:
                        - uses: acts.core.irq
                          key: act2
                    - id: step3
                      acts:
                        - uses: acts.core.msg
                          key: msg1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    let act = step.acts.first().unwrap();
    assert_eq!(act.catches.len(), 2);

    let catch = act.catches.first().unwrap();
    assert_eq!(catch.on.as_ref().unwrap(), "err1");
    assert_eq!(catch.steps.len(), 0);

    let catch = act.catches.get(1).unwrap();
    assert_eq!(catch.on, None);
    assert_eq!(catch.steps.len(), 2);
}
