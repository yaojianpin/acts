mod block;
mod call;
mod catch;
mod chain;
mod r#do;
mod each;
mod expose;
mod hooks;
mod r#if;
mod msg;
mod pack;
mod req;
mod set;
mod setup;
mod timeout;

use crate::Act;

#[test]
fn model_act_parse_nest() {
    let text = r#"
    act: each
    in: "[\"a\", \"b\"]"
    then:
        - act: msg
          key: msg1
        - act: set
          inputs:
            a: 10
        - act: each
          in: "[\"a\", \"b\"]"
          then:
            - act: msg
              inputs:
                key: msg2
            - act: if
              on: $("a") > 0
              then:
                - act: msg
                  key: msg3
    "#;
    assert!(serde_yaml::from_str::<Act>(text).is_ok());
}

#[test]
fn model_act_to_json() {
    let text = r#"
    - act: each
      in: "[\"a\", \"b\"]"
      then:
          - act: msg
            key: msg1
          - act: each
            in: "[\"a\", \"b\"]"
            then:
              - act: msg
                key: msg2
    - act: msg
      key: msg2
    "#;

    let stms: Vec<Act> = serde_yaml::from_str(text).unwrap();
    let ret = serde_json::to_string(&stms);
    assert!(ret.is_ok());
}
