use crate::{ActSchema, Variant, VariantTypes, Vars};
use serde_json::json;

#[test]
fn model_vars_new() {
    let vars = Vars::new();
    assert_eq!(vars.len(), 0);
}

#[test]
fn model_vars_from() {
    let vars = json!({ "a": 10 });
    let vars: Vars = vars.into();
    assert_eq!(vars.get::<i32>("a").unwrap(), 10);
}

#[test]
fn model_vars_insert() {
    let mut vars = Vars::new();
    vars.insert("a".to_string(), json!(10));
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
}

#[test]
fn model_vars_set() {
    let mut vars = Vars::new();
    vars.set("a", json!(10));
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
}

#[test]
fn model_vars_set_vec() {
    let mut vars = Vars::new();
    vars.set("a", ["a"]);
    assert_eq!(vars.get::<Vec<String>>("a").unwrap(), ["a"]);
}

#[test]
fn model_vars_remove() {
    let mut vars = Vars::new();
    vars.set("a", json!(10));
    vars.remove("a");
    assert_eq!(vars.get_value("a"), None);
}

#[test]
fn model_vars_with() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.len(), 2);
    assert_eq!(vars.get_value("a").unwrap(), &json!(10));
    assert_eq!(vars.get_value("b").unwrap(), &json!("text"));
}

#[test]
fn model_vars_iter() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.iter().len(), 2);
}

#[test]
fn model_vars_iter_mut() {
    let mut vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(vars.iter_mut().len(), 2);
}

#[test]
fn model_vars_to_string() {
    let vars = Vars::new().with("a", 10).with("b", "text");
    assert_eq!(
        vars.to_string(),
        json!({ "a": 10, "b": "text" }).to_string()
    );
}

#[test]
fn model_vars_value_default() {
    let v = ActSchema::default();
    assert!(v.is_empty());
}

#[test]
fn model_vars_value_var_create() {
    let v = ActSchema::Simple(Variant::create("test", 42));
    let var = v.simple().unwrap();
    assert_eq!(var.name, "test");
    assert_eq!(var.value, json!(42));
}

#[test]
fn model_vars_value_var_properties() {
    let v = ActSchema::Simple(
        Variant::new()
            .name("name1")
            .value(42)
            .r#type(VariantTypes::Number)
            .required(true)
            .title("Test Name1")
            .desc("This is a test variant"),
    );
    let var = v.simple().unwrap();
    assert_eq!(var.name, "name1");
    assert_eq!(var.value, json!(42));
    assert_eq!(var.title, "Test Name1");
    assert_eq!(var.r#type, VariantTypes::Number);
    assert!(var.required);
    assert_eq!(var.desc, "This is a test variant");
}

#[test]
fn model_vars_value_vars() {
    let v = ActSchema::Multiple(vec![
        Variant::create("name1", 42),
        Variant::create("name2", "value2"),
        Variant::create("name3", [1, 2]),
        Variant::create("name4", json!({"key": "value"})),
        Variant::create("name5", true),
    ]);
    let vars = v.multiple().unwrap();
    assert_eq!(vars.len(), 5);

    let var = &vars[0];
    assert_eq!(var.name, "name1");
    assert_eq!(var.value, json!(42));

    let var = &vars[1];
    assert_eq!(var.name, "name2");
    assert_eq!(var.value, json!("value2"));

    let var = &vars[2];
    assert_eq!(var.name, "name3");
    assert_eq!(var.value, json!([1, 2]));

    let var = &vars[3];
    assert_eq!(var.name, "name4");
    assert_eq!(var.value, json!({"key": "value"}));

    let var = &vars[4];
    assert_eq!(var.name, "name5");
    assert_eq!(var.value, json!(true));
}

#[test]
fn model_vars_value_schema() {
    let v = ActSchema::Multiple(vec![
        Variant::new().name("name1").r#type(VariantTypes::Number),
        Variant::new().name("name2").r#type(VariantTypes::String),
        Variant::new().name("name3").r#type(VariantTypes::Array),
        Variant::new().name("name4").r#type(VariantTypes::Object),
        Variant::new().name("name5").r#type(VariantTypes::Boolean),
    ]);
    let schema = v.schema();
    let data = json!({
        "name1": 42,
        "name2": "value2",
        "name3": [1, 2],
        "name4": json!({"key": "value"}),
        "name5": true
    });
    let result = jsonschema::validate(&schema, &data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_none() {
    let v = ActSchema::None;

    let data = json!({
        "name1": 42,
        "name2": "value2",
        "name3": [1, 2],
        "name4": json!({"key": "value"}),
        "name5": true
    });
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_vars() {
    let v = ActSchema::Multiple(vec![
        Variant::new().name("name1").r#type(VariantTypes::Number),
        Variant::new().name("name2").r#type(VariantTypes::String),
        Variant::new().name("name3").r#type(VariantTypes::Array),
        Variant::new().name("name4").r#type(VariantTypes::Object),
        Variant::new().name("name5").r#type(VariantTypes::Boolean),
    ]);

    let data = json!({
        "name1": 42,
        "name2": "value2",
        "name3": [1, 2],
        "name4": json!({"key": "value"}),
        "name5": true
    });
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_primitive_number() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::Number));

    let data = json!(5);
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_primitive_string() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::String));

    let data = json!("test string");
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_primitive_boolean() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::Boolean));

    let data = json!(false);
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_primitive_array() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::Array));

    let data = json!([1, 2, 3]);
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_ok_primitive_object() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::Object));

    let data = json!({ "key": "value"});
    let result = v.validate(&data);
    assert!(
        result.is_ok(),
        "Schema validation failed: {:?}",
        result.err()
    );
}

#[test]
fn model_vars_value_validate_err_primitive() {
    let v = ActSchema::Simple(Variant::new().name("name1").r#type(VariantTypes::Number));

    let data = json!("not a number");
    let result = v.validate(&data);
    assert!(result.is_err());
}

#[test]
fn model_vars_value_validate_err_required() {
    let v = ActSchema::Multiple(vec![
        Variant::new()
            .name("name1")
            .r#type(VariantTypes::Number)
            .required(true),
        Variant::new()
            .name("name2")
            .r#type(VariantTypes::String)
            .required(false),
    ]);

    let data = json!({
        "name2": "v2",
    });
    let result = v.validate(&data);
    assert!(result.is_err());
}

#[test]
fn model_vars_value_validate_err_additional() {
    let v = ActSchema::Multiple(vec![
        Variant::new()
            .name("name1")
            .r#type(VariantTypes::Number)
            .required(false),
    ]);

    let data = json!({
        "name2": "v2",
    });
    let result = v.validate(&data);
    assert!(result.is_err());
}

#[test]
fn model_vars_value_validate_err_type() {
    let v = ActSchema::Multiple(vec![
        Variant::new()
            .name("name1")
            .r#type(VariantTypes::Number)
            .required(false),
    ]);

    let data = json!({
        "name1": "v1",
    });
    let result = v.validate(&data);
    assert!(result.is_err());
}
#[test]
fn model_vars_value_validate_ok_untyped() {
    // a variant declared without `type` (a name-only `exposes` entry) is
    // not forced to string — every runtime JSON value passes and keeps its
    // own type
    let v = ActSchema::Multiple(vec![Variant::new().name("name1")]);
    let schema = v.schema();
    assert!(schema["properties"]["name1"].get("type").is_none());

    for data in [
        json!({"name1": 42}),
        json!({"name1": "text"}),
        json!({"name1": true}),
        json!({"name1": [1, 2]}),
        json!({"name1": {"k": "v"}}),
        json!({"name1": null}),
    ] {
        let result = v.validate(&data);
        assert!(
            result.is_ok(),
            "untyped validation failed for {data:?}: {result:?}"
        );
    }
}

#[test]
fn model_schema_new_none_and_schema() {
    let schema = ActSchema::new();
    assert!(schema.is_empty());
    assert_eq!(schema.schema(), json!({}));
    assert!(schema.simple().is_none());
    assert!(schema.multiple().is_none());
}

#[test]
fn model_schema_wrong_variant_accessors_return_none() {
    let simple = ActSchema::Simple(Variant::create("a", 1));
    assert!(simple.multiple().is_none());

    let multiple = ActSchema::Multiple(vec![Variant::create("a", 1)]);
    assert!(multiple.simple().is_none());
}

#[test]
fn model_vars_from_iter_and_collections() {
    let vars: Vars = [("a".to_string(), json!(1)), ("b".to_string(), json!(2))]
        .into_iter()
        .collect();
    assert_eq!(vars.len(), 2);
    assert_eq!(vars.get::<i32>("b").unwrap(), 2);

    let map = serde_json::Map::from_iter([("c".to_string(), json!(3))]);
    let vars: Vars = map.into();
    assert_eq!(vars.get::<i32>("c").unwrap(), 3);

    let vars: Vars = vec![Variant::create("d", 4), Variant::create("e", 5)].into();
    assert_eq!(vars.get::<i32>("d").unwrap(), 4);
    assert_eq!(vars.get::<i32>("e").unwrap(), 5);
}

#[test]
fn model_vars_from_non_object_value_is_empty() {
    let vars: Vars = json!([1, 2, 3]).into();
    assert!(vars.is_empty());

    let value: serde_json::Value = vars.into();
    assert_eq!(value, json!({}));
}

#[test]
fn model_vars_extend_and_append() {
    let base = Vars::new().with("a", 1).with("b", 2);
    let merged = base.extend(Vars::new().with("b", 20).with("c", 3));
    assert_eq!(merged.get::<i32>("a").unwrap(), 1);
    assert_eq!(merged.get::<i32>("b").unwrap(), 20);
    assert_eq!(merged.get::<i32>("c").unwrap(), 3);

    let mut left = Vars::new().with("a", 1).with("shared", 1);
    let mut right = Vars::new().with("b", 2).with("shared", 2);
    left.append(&mut right);
    assert_eq!(left.get::<i32>("a").unwrap(), 1);
    assert_eq!(left.get::<i32>("b").unwrap(), 2);
    assert_eq!(left.get::<i32>("shared").unwrap(), 2);
    assert!(right.is_empty());
}

#[test]
fn model_vars_get_and_pop_type_mismatch() {
    let mut vars = Vars::new().with("a", "text");
    assert!(vars.get::<i32>("a").is_none());
    assert!(vars.get::<i32>("missing").is_none());
    assert!(vars.pop::<i32>("missing").is_none());
    // the value is removed even when it cannot be decoded as the asked type
    assert!(vars.pop::<i32>("a").is_none());
    assert!(vars.get_value("a").is_none());

    let mut vars = Vars::new().with("a", 7);
    assert_eq!(vars.pop::<i32>("a"), Some(7));
}

#[test]
fn model_vars_debug_iterators_and_deref_mut() {
    let mut vars = Vars::new().with("a", 1).with("b", 2);
    assert_eq!(vars.to_string(), serde_json::to_string(&vars).unwrap());
    assert!(format!("{vars:?}").contains("\"a\":1"));

    let owned: Vec<(String, serde_json::Value)> = (&vars).into_iter().collect();
    assert_eq!(owned.len(), 2);

    let mut count = 0;
    for (_k, v) in &mut vars {
        if v.as_i64() == Some(1) {
            *v = json!(10);
        }
        count += 1;
    }
    assert_eq!(count, 2);
    assert_eq!(vars.get::<i32>("a").unwrap(), 10);

    // DerefMut gives Map's own mutation api
    vars.insert("c".to_string(), json!(3));
    assert_eq!(vars.len(), 3);
}
