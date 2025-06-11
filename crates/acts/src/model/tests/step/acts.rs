use crate::Workflow;

#[test]
fn model_step_yml_acts() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - uses: acts.core.irq
              params:
                a: 5
            - uses: acts.core.msg
              inputs:
                a: 10
              params:
                b: '{{ a }}'
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.acts.len(), 2);
}
