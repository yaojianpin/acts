use crate::Workflow;

#[test]
fn model_step_yml_acts() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          acts:
            - !set
              a: 5
            - !req
              id: act1
              inputs:
                b: ${ $("5") }
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.get(0).unwrap();
    assert_eq!(step.acts.len(), 2);
}
