//! Errors the evaluator reports.

use std::fmt;

/// Everything `acts-expr` can fail with.
///
/// A parse error carries the byte offset in the source it stopped at; an
/// evaluation error names the operation and the value type it could not apply
/// to, because the same expression can be valid for one context and not for
/// the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The source is not a well-formed expression.
    Parse { message: String, pos: usize },
    /// An identifier is not in the context.
    Unknown { name: String },
    /// A member of an object (or an index of a list) does not exist.
    Missing { name: String },
    /// A list index is negative or past the end, with the length it was read
    /// against.
    OutOfRange { index: i64, len: usize },
    /// An operand or receiver has no type for the operation.
    Type {
        /// The operation as it reads in the source, e.g. `'+'` or `'<'`.
        operation: &'static str,
        /// The value's type: `null`, `int`, `float`, `string`, `bool`, `list`,
        /// `map` or `function`.
        got: &'static str,
    },
    /// An arithmetic operation overflowed an `i64`.
    Overflow { operation: &'static str },
    /// A division or remainder had a zero divisor.
    DivideByZero { operation: &'static str },
    /// A native function (an injected method) failed.
    Function { name: String, message: String },
    /// The receiver of a method call is not an object and no function of that
    /// name is in the context.
    UnknownMethod { name: String },
    /// A value could not cross the `json` bridge.
    Convert { message: String },
}

/// The crate's result type: the error is `Error` unless a caller names
/// another.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// The byte offset a parse error stopped at, for a host that wants to
    /// point at it. `None` for every error raised at evaluation time.
    pub fn position(&self) -> Option<usize> {
        match self {
            Error::Parse { pos, .. } => Some(*pos),
            _ => None,
        }
    }

    pub(crate) fn parse(message: impl Into<String>, pos: usize) -> Error {
        Error::Parse {
            message: message.into(),
            pos,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { message, pos } => write!(f, "parse error at byte {pos}: {message}"),
            Error::Unknown { name } => write!(f, "unknown variable '{name}'"),
            Error::Missing { name } => write!(f, "no such member '{name}'"),
            Error::OutOfRange { index, len } => {
                write!(f, "index {index} is out of range for a list of {len}")
            }
            Error::Type { operation, got } => {
                write!(f, "no {operation} for a {got} value")
            }
            Error::Overflow { operation } => write!(f, "'{operation}' overflowed an int"),
            Error::DivideByZero { operation } => write!(f, "'{operation}' by zero"),
            Error::Function { name, message } => {
                write!(f, "'{name}' failed: {message}")
            }
            Error::UnknownMethod { name } => write!(f, "no such method '{name}'"),
            Error::Convert { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_is_only_present_for_parse_errors() {
        let parse = Error::parse("expected a value", 7);
        assert_eq!(parse.position(), Some(7));
        assert_eq!(
            parse,
            Error::Parse {
                message: "expected a value".to_string(),
                pos: 7
            }
        );

        for error in [
            Error::Unknown {
                name: "x".to_string(),
            },
            Error::Missing {
                name: "x".to_string(),
            },
            Error::OutOfRange { index: 1, len: 0 },
            Error::Type {
                operation: "+",
                got: "int",
            },
            Error::Overflow { operation: "+" },
            Error::DivideByZero { operation: "/" },
            Error::Function {
                name: "f".to_string(),
                message: "failed".to_string(),
            },
            Error::UnknownMethod {
                name: "m".to_string(),
            },
            Error::Convert {
                message: "bad".to_string(),
            },
        ] {
            assert_eq!(error.position(), None);
        }
    }

    #[test]
    fn display_names_the_problem() {
        assert_eq!(
            Error::parse("expected a value", 4).to_string(),
            "parse error at byte 4: expected a value"
        );
        assert_eq!(
            Error::Unknown {
                name: "count".to_string()
            }
            .to_string(),
            "unknown variable 'count'"
        );
        assert_eq!(
            Error::Missing {
                name: "retries".to_string()
            }
            .to_string(),
            "no such member 'retries'"
        );
        assert_eq!(
            Error::OutOfRange { index: 3, len: 2 }.to_string(),
            "index 3 is out of range for a list of 2"
        );
        assert_eq!(
            Error::Type {
                operation: "+",
                got: "string"
            }
            .to_string(),
            "no + for a string value"
        );
        assert_eq!(
            Error::Overflow { operation: "*" }.to_string(),
            "'*' overflowed an int"
        );
        assert_eq!(
            Error::DivideByZero { operation: "%" }.to_string(),
            "'%' by zero"
        );
        assert_eq!(
            Error::Function {
                name: "upper".to_string(),
                message: "takes 1 argument(s), got 0".to_string()
            }
            .to_string(),
            "'upper' failed: takes 1 argument(s), got 0"
        );
        assert_eq!(
            Error::UnknownMethod {
                name: "upper".to_string()
            }
            .to_string(),
            "no such method 'upper'"
        );
        assert_eq!(
            Error::Convert {
                message: "not json".to_string()
            }
            .to_string(),
            "not json"
        );
    }

    #[test]
    fn it_is_a_std_error_with_no_source() {
        let error = Error::Unknown {
            name: "x".to_string(),
        };
        let std_error: &dyn std::error::Error = &error;
        assert!(std_error.source().is_none());
        assert_eq!(format!("{std_error}"), "unknown variable 'x'");
    }
}
