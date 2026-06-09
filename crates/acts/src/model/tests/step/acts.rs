use crate::Workflow;

#[test]
fn model_step_yml_uses() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          uses: acts.core.irq
          params:
            a: 5
        - id: step2
          uses: acts.core.msg
          params:
            b: '{{ a }}'
    "#;
    let m = Workflow::from_yml(text).unwrap();
    assert_eq!(m.steps.len(), 2);
}
