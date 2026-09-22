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
