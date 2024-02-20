use crate::Act;
use serde::{Deserialize, Serialize};

#[test]
fn model_act_parse_expose() {
    let text = r#"
    !expose
    a: 1
    b: abc
    "#;
    if let Act::Expose(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.get::<i32>("a").unwrap(), 1);
        assert_eq!(stmt.get::<String>("b").unwrap(), "abc");
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_expose_null() {
    let text = r#"
    !expose
    a:
    b:
    "#;
    if let Act::Expose(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(stmt.get::<()>("a").unwrap(), ());
        assert_eq!(stmt.get::<()>("b").unwrap(), ());
    } else {
        assert!(false);
    }
}

#[test]
fn model_act_parse_expose_obj() {
    #[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
    struct TestModel {
        a: i32,
        b: String,
    }
    let text = r#"
    !expose
    a:
     a: 1
     b: abc
    "#;
    if let Act::Expose(stmt) = serde_yaml::from_str(text).unwrap() {
        assert_eq!(
            stmt.get::<TestModel>("a").unwrap(),
            TestModel {
                a: 1,
                b: "abc".to_string()
            }
        );
    } else {
        assert!(false);
    }
}
