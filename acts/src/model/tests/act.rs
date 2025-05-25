mod catch;
mod hooks;
mod setup;
mod timeout;

use crate::Act;

#[test]
fn model_act_parse_nest() {
    let text = r#"
    uses: acts.transform.parallel
    in: "[\"a\", \"b\"]"
    acts:
        - uses: acts.core.msg
          key: msg1
        - uses: acts.core.set
          inputs:
            a: 10
        - act: acts.transform.parallel
          in: "[\"a\", \"b\"]"
          acts:
            - uses: acts.core.msg
              inputs:
                key: msg2
            - uses: acts.core.msg
              if: $("a") > 0
              key: msg3

    "#;
    assert!(serde_yaml::from_str::<Act>(text).is_ok());
}

#[test]
fn model_act_to_json() {
    let text = r#"
    - uses: acts.transform.parallel
      in: "[\"a\", \"b\"]"
      acts:
          - uses: acts.core.msg
            key: msg1
          - uses: acts.transform.parallel
            in: "[\"a\", \"b\"]"
            acts:
              - uses: acts.core.msg
                key: msg2
    - uses: acts.core.msg
      key: msg2
    "#;

    let stms: Vec<Act> = serde_yaml::from_str(text).unwrap();
    let ret = serde_json::to_string(&stms);
    assert!(ret.is_ok());
}
