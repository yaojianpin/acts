use crate::Vars;

#[test]
fn value_accessors_return_none_on_type_mismatch() {
    let vars = Vars::new()
        .with("num", 42)
        .with("text", "hello")
        .with("flag", true);

    assert_eq!(vars.value_str("text"), Some("hello"));
    assert_eq!(vars.value_number("num"), Some(42.0));

    // A present key of the wrong protocol type is a miss, not a panic.
    assert_eq!(vars.value_str("flag"), None);
    assert_eq!(vars.value_number("text"), None);

    assert_eq!(vars.value_str("missing"), None);
    assert_eq!(vars.value_number("missing"), None);
}
