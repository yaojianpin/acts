//! The methods a value carries of its own: the small string and list surface
//! an expression needs without a library behind it.
//!
//! They are *methods* — `name.startsWith('a')`, not `startsWith(name, 'a')` —
//! because that is how an expression reads: the method belongs to the value it
//! applies to. A function of the same name injected in the context, or one
//! carried by the receiver object itself, is found first and wins.

use crate::value::Value;
use crate::{Error, Result};

/// Call `receiver.name(args)`.
///
/// `Ok(None)` when `name` is not a built-in method at all, which the evaluator
/// reports as an unknown method; `Err` when it is one but the receiver or an
/// argument has the wrong type.
pub(crate) fn call(name: &str, receiver: &Value, args: &[Value]) -> Result<Option<Value>> {
    let value = match name {
        // `length()` counts characters for a string (not bytes), items for a
        // list, entries for a map.
        "length" => {
            arity(name, args, 0)?;
            match receiver {
                Value::Str(text) => Value::from(text.chars().count()),
                Value::List(items) => Value::from(items.len()),
                Value::Map(map) => Value::from(map.len()),
                other => return Err(receiver_error(name, other, "a string, list or map")),
            }
        }

        "startsWith" | "endsWith" => {
            arity(name, args, 1)?;
            let Value::Str(text) = receiver else {
                return Err(receiver_error(name, receiver, "a string"));
            };
            let needle = text_arg(name, args, 0)?;

            Value::Bool(if name == "startsWith" {
                text.starts_with(needle.as_str())
            } else {
                text.ends_with(needle.as_str())
            })
        }

        // The position of the first occurrence, counted in characters so it is
        // the same unit `length()` reports, or -1 when the string does not
        // contain it.
        "indexOf" => {
            arity(name, args, 1)?;
            let Value::Str(text) = receiver else {
                return Err(receiver_error(name, receiver, "a string"));
            };
            let needle = text_arg(name, args, 0)?;

            Value::Int(
                text.find(needle.as_str())
                    .map_or(-1, |byte| text[..byte].chars().count() as i64),
            )
        }

        // A substring of a string, an element of a list (compared by value).
        "contains" => {
            arity(name, args, 1)?;
            let needle = &args[0];
            match receiver {
                Value::Str(text) => Value::Bool(text.contains(text_arg(name, args, 0)?.as_str())),
                Value::List(items) => Value::Bool(items.iter().any(|item| item.equals(needle))),
                other => return Err(receiver_error(name, other, "a string or list")),
            }
        }

        #[cfg(feature = "regex")]
        "is_match" => {
            arity(name, args, 1)?;
            let Value::Str(text) = receiver else {
                return Err(receiver_error(name, receiver, "a string"));
            };
            let pattern = text_arg(name, args, 0)?;
            let regex = regex::Regex::new(&pattern).map_err(|err| Error::Function {
                name: name.to_string(),
                message: format!("'{pattern}' is not a regular expression: {err}"),
            })?;

            Value::Bool(regex.is_match(text))
        }

        _ => return Ok(None),
    };

    Ok(Some(value))
}

fn arity(name: &str, args: &[Value], expected: usize) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }

    Err(Error::Function {
        name: name.to_string(),
        message: format!("takes {expected} argument(s), got {}", args.len()),
    })
}

fn text_arg(name: &str, args: &[Value], index: usize) -> Result<String> {
    match args.get(index) {
        Some(Value::Str(text)) => Ok(text.to_string()),
        Some(other) => Err(Error::Function {
            name: name.to_string(),
            message: format!(
                "argument {} must be a string, got {}",
                index + 1,
                other.type_name()
            ),
        }),
        None => Err(Error::Function {
            name: name.to_string(),
            message: format!("argument {} is missing", index + 1),
        }),
    }
}

fn receiver_error(name: &str, receiver: &Value, expected: &str) -> Error {
    Error::Function {
        name: name.to_string(),
        message: format!("expects {expected}, got {}", receiver.type_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(name: &str, message: &str) -> Error {
        Error::Function {
            name: name.to_string(),
            message: message.to_string(),
        }
    }

    fn call_err(name: &str, receiver: &Value, args: &[Value]) -> Error {
        call(name, receiver, args).expect_err(name)
    }

    #[test]
    fn length_counts_items_entries_and_characters() {
        assert_eq!(
            call("length", &Value::from("héllo"), &[]).unwrap(),
            Some(Value::from(5))
        );
        assert_eq!(
            call("length", &Value::list([1, 2, 3]), &[]).unwrap(),
            Some(Value::from(3))
        );
        assert_eq!(
            call("length", &Value::map([("a", 1), ("b", 2)]), &[]).unwrap(),
            Some(Value::from(2))
        );
        assert_eq!(
            call_err("length", &Value::from(1), &[]),
            function("length", "expects a string, list or map, got int")
        );
        assert_eq!(
            call_err("length", &Value::from("x"), &[Value::Null]),
            function("length", "takes 0 argument(s), got 1")
        );
    }

    #[test]
    fn starts_with_and_ends_with() {
        let text = Value::from("workflow");

        assert_eq!(
            call("startsWith", &text, &[Value::from("work")]).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            call("startsWith", &text, &[Value::from("flow")]).unwrap(),
            Some(Value::Bool(false))
        );
        assert_eq!(
            call("endsWith", &text, &[Value::from("flow")]).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            call("endsWith", &text, &[Value::from("work")]).unwrap(),
            Some(Value::Bool(false))
        );

        let expected = function("startsWith", "expects a string, got int");
        assert_eq!(
            call_err("startsWith", &Value::from(1), &[Value::Null]),
            expected
        );
        assert_eq!(
            call_err("endsWith", &Value::from(1), &[Value::Null]),
            function("endsWith", "expects a string, got int")
        );
        assert_eq!(
            call_err("endsWith", &text, &[]),
            function("endsWith", "takes 1 argument(s), got 0")
        );
        assert_eq!(
            call_err("startsWith", &text, &[Value::Int(1)]),
            function("startsWith", "argument 1 must be a string, got int")
        );
    }

    #[test]
    fn index_of_counts_characters() {
        assert_eq!(
            call("indexOf", &Value::from("héllo"), &[Value::from("llo")]).unwrap(),
            Some(Value::Int(2))
        );
        assert_eq!(
            call("indexOf", &Value::from("abc"), &[Value::from("z")]).unwrap(),
            Some(Value::Int(-1))
        );
        assert_eq!(
            call_err("indexOf", &Value::from(1), &[Value::from("a")]),
            function("indexOf", "expects a string, got int")
        );
        assert_eq!(
            call_err("indexOf", &Value::from("abc"), &[]),
            function("indexOf", "takes 1 argument(s), got 0")
        );
        assert_eq!(
            call_err("indexOf", &Value::from("abc"), &[Value::Bool(true)]),
            function("indexOf", "argument 1 must be a string, got bool")
        );
    }

    #[test]
    fn contains_works_on_strings_and_lists() {
        assert_eq!(
            call("contains", &Value::from("abcdef"), &[Value::from("cd")]).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            call(
                "contains",
                &Value::list([Value::Int(1), Value::Int(2)]),
                &[Value::Int(2)]
            )
            .unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            call("contains", &Value::list([Value::Int(1)]), &[Value::Int(2)]).unwrap(),
            Some(Value::Bool(false))
        );
        assert_eq!(
            call_err("contains", &Value::from(1), &[Value::Int(1)]),
            function("contains", "expects a string or list, got int")
        );
        assert_eq!(
            call_err("contains", &Value::from("abc"), &[]),
            function("contains", "takes 1 argument(s), got 0")
        );
        assert_eq!(
            call_err("contains", &Value::from("abc"), &[Value::Bool(true)]),
            function("contains", "argument 1 must be a string, got bool")
        );
    }

    #[test]
    fn missing_arguments_and_unknown_names() {
        // `call` checks arity first, so the "missing" arm is only reachable
        // directly.
        assert_eq!(
            text_arg("f", &[], 0).unwrap_err(),
            function("f", "argument 1 is missing")
        );
        assert_eq!(
            text_arg("f", &[Value::Int(1)], 0).unwrap_err(),
            function("f", "argument 1 must be a string, got int")
        );
        assert_eq!(
            text_arg("f", &[Value::from("a")], 1).unwrap_err(),
            function("f", "argument 2 is missing")
        );

        assert_eq!(
            call("upper", &Value::from("a"), &[]).unwrap(),
            None,
            "an unknown method is not this module's to report"
        );
    }

    #[cfg(feature = "regex")]
    #[test]
    fn is_match_needs_a_pattern_and_a_string_receiver() {
        assert_eq!(
            call(
                "is_match",
                &Value::from("ERR-404"),
                &[Value::from(r"^ERR-[0-9]+$")]
            )
            .unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            call("is_match", &Value::from("ERR-404"), &[Value::from("^OK")]).unwrap(),
            Some(Value::Bool(false))
        );
        assert_eq!(
            call_err("is_match", &Value::from(1), &[Value::from("a")]),
            function("is_match", "expects a string, got int")
        );
        assert_eq!(
            call_err("is_match", &Value::from("a"), &[]),
            function("is_match", "takes 1 argument(s), got 0")
        );
        assert_eq!(
            call_err("is_match", &Value::from("a"), &[Value::Int(1)]),
            function("is_match", "argument 1 must be a string, got int")
        );

        let err = call_err("is_match", &Value::from("a"), &[Value::from("(")]);
        match err {
            Error::Function { name, message } => {
                assert_eq!(name, "is_match");
                assert!(message.contains("is not a regular expression"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }
}
