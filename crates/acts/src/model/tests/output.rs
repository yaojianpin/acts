use crate::{Output, OutputType, Outputs};
use serde_json::json;

#[test]
fn model_output_type() {
    let output = Output {
        r#type: OutputType::String,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("type").unwrap(), "String");

    let output = Output {
        r#type: OutputType::Number,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("type").unwrap(), "Number");

    let output = Output {
        r#type: OutputType::Bool,
        ..Default::default()
    };

    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("type").unwrap(), "Bool");

    let output = Output {
        r#type: OutputType::Object,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("type").unwrap(), "Object");

    let output = Output {
        r#type: OutputType::Array,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("type").unwrap(), "Array");
}

#[test]
fn model_output_required() {
    let output = Output {
        required: true,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("required").unwrap(), true);

    let output = Output {
        required: false,
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("required").unwrap(), false);
}

#[test]
fn model_output_default() {
    let output = Output {
        default: "abc".into(),
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("default").unwrap(), "abc");

    let output = Output {
        default: 10.into(),
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("default").unwrap(), 10);

    let output = Output {
        default: json!(null),
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert!(value.get("default").unwrap().is_null());

    let output = Output {
        default: json!([]),
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("default").unwrap(), &json!([]));

    let output = Output {
        default: json!({ "a": 5 }),
        ..Default::default()
    };
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(value.get("default").unwrap(), &json!({ "a": 5 }));
}

#[test]
fn model_outputs_one() {
    let mut outputs = Outputs::default();
    outputs.push(
        "abc",
        &Output {
            default: "abc".into(),
            required: true,
            r#type: OutputType::String,
        },
    );

    let value = serde_json::to_value(&outputs).unwrap();

    let output = value.get("abc").unwrap();
    assert_eq!(output.get("default").unwrap(), "abc");
    assert_eq!(output.get("required").unwrap(), true);
    assert_eq!(output.get("type").unwrap(), "String");
}

#[test]
fn model_outputs_many() {
    let mut outputs = Outputs::default();
    outputs.push(
        "a",
        &Output {
            default: "abc".into(),
            required: true,
            r#type: OutputType::String,
        },
    );

    outputs.push(
        "b",
        &Output {
            default: json!(0),
            required: false,
            r#type: OutputType::Number,
        },
    );

    outputs.push(
        "c",
        &Output {
            default: json!([]),
            required: true,
            r#type: OutputType::Array,
        },
    );

    outputs.push(
        "d",
        &Output {
            default: json!(null),
            required: true,
            r#type: OutputType::Object,
        },
    );

    let value = serde_json::to_value(&outputs).unwrap();

    let output = value.get("a").unwrap();
    assert_eq!(output.get("default").unwrap(), "abc");
    assert_eq!(output.get("required").unwrap(), true);
    assert_eq!(output.get("type").unwrap(), "String");

    let output = value.get("b").unwrap();
    assert_eq!(output.get("default").unwrap(), &json!(0));
    assert_eq!(output.get("required").unwrap(), false);
    assert_eq!(output.get("type").unwrap(), "Number");

    let output = value.get("c").unwrap();
    assert_eq!(output.get("default").unwrap(), &json!([]));
    assert_eq!(output.get("required").unwrap(), true);
    assert_eq!(output.get("type").unwrap(), "Array");

    let output = value.get("d").unwrap();
    assert_eq!(output.get("default").unwrap(), &json!(null));
    assert_eq!(output.get("required").unwrap(), true);
    assert_eq!(output.get("type").unwrap(), "Object");
}
