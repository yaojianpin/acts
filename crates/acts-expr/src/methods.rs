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
