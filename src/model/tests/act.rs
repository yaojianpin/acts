mod chain;
mod cmd;
mod each;
mod expose;
mod hooks;
mod r#if;
mod msg;
mod pack;
mod req;
mod set;
mod r#use;

use crate::Act;

#[test]
fn model_act_parse_nest() {
    let text = r#"
    !each
    in: "[\"a\", \"b\"]"
    run:
        - !msg
          id: msg1
        - !set
          a: 10
        - !each
          in: "[\"a\", \"b\"]"
          run:
            - !msg
              id: msg2
            - !if
              on: env.get("a") > 0
              then:
                - !msg
                  id: msg3
    "#;
    assert!(serde_yaml::from_str::<Act>(text).is_ok());
}

#[test]
fn model_act_to_json() {
    let text = r#"
    - !each
        in: "[\"a\", \"b\"]"
        run:
            - !msg
              id: msg1
            - !each
              in: "[\"a\", \"b\"]"
              run:
                - !msg
                  id: msg2
    - !msg
      id: msg2
    "#;

    let stms: Vec<Act> = serde_yaml::from_str(text).unwrap();
    let ret = serde_json::to_string(&stms);
    assert_eq!(ret.is_ok(), true);
}
