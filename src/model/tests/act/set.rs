use serde::{Deserialize, Serialize};

use crate::ActFn;

#[test]
fn model_act_set_parse_primary() {
    let text = r#"
    !set
    a: 1
    b: abc
    "#;
    if let ActFn::Set(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.get::<i32>("a").unwrap(), 1);
        assert_eq!(stmt.get::<String>("b").unwrap(), "abc");
    } else {
        panic!();
    }
}

#[test]
fn model_act_set_parse_arr() {
    let text = r#"
    !set
    a: ["a", "b"]
    "#;
    if let ActFn::Set(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.get::<Vec<String>>("a").unwrap(), ["a", "b"]);
    } else {
        panic!();
    }
}

#[test]
fn model_act_set_parse_obj() {
    #[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
    struct TestModel {
        a: i32,
        b: String,
    }

    let text = r#"
    !set
    a:
      a: 1
      b: abc
    "#;
    if let ActFn::Set(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(
            stmt.get::<TestModel>("a").unwrap(),
            TestModel {
                a: 1,
                b: "abc".to_string()
            }
        );
    } else {
        panic!();
    }
}
