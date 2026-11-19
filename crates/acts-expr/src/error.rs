//! Errors the evaluator reports.

use std::fmt;

/// The kind of runtime budget an expression exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BudgetKind {
    Steps,
    Allocation,
    Output,
    FunctionCalls,
    CollectionItems,
}

impl BudgetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetKind::Steps => "step",
            BudgetKind::Allocation => "allocation",
            BudgetKind::Output => "output",
            BudgetKind::FunctionCalls => "function-call",
            BudgetKind::CollectionItems => "collection-item",
        }
    }
}

/// Everything `acts-expr` can fail with.
///
/// A parse error carries the byte offset in the source it stopped at; an
/// evaluation error names the operation and the value type it could not apply
/// to, because the same expression can be valid for one context and not for
/// the next. Error text is bounded: a source, value, regex pattern or host
/// function message is never echoed in full.
#[derive(Clone, PartialEq, Eq)]
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
    /// A compile-time limit (source, literal, identifier, regex) was exceeded.
    TooLarge { what: &'static str, limit: usize },
    /// The source produced more tokens than the profile allows.
    TooManyTokens { limit: usize },
    /// The source produced more AST nodes than the profile allows.
    TooManyNodes { limit: usize },
    /// The logical AST height (including binary chains) exceeded the profile.
    TooDeep { limit: usize },
    /// A call carried more arguments than the profile allows.
    TooManyArguments { limit: usize },
    /// A runtime budget was exhausted.
    BudgetExceeded { kind: BudgetKind, limit: u64 },
    /// The evaluation deadline passed.
    DeadlineExceeded,
    /// The evaluation was cancelled.
    Cancelled,
    /// A value exceeded a [`crate::ValueLimits`] boundary.
    ValueLimit { what: &'static str, limit: usize },
    /// A native function panicked; the evaluator caught it and stayed alive.
    FunctionPanicked { name: String },
    /// A function declared an external effect that the profile forbids.
    FunctionRejected { name: String, reason: &'static str },
    /// The result does not have the type the caller required.
    ResultType {
        expected: &'static str,
        got: &'static str,
    },
    /// A float literal, operation or injected value is NaN or infinite.
    NonFinite { operation: &'static str },
}

/// The crate's result type: the error is `Error` unless a caller names
/// another.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The longest error text this crate emits (or echoes from a host).
const MAX_ERROR_TEXT: usize = 1024;

impl Error {
    /// The byte offset a parse error stopped at, for a host that wants to
    /// point at it. `None` for every error raised at compile/evaluation time.
    pub fn position(&self) -> Option<usize> {
        match self {
            Error::Parse { pos, .. } => Some(*pos),
            _ => None,
        }
    }

    /// A stable machine-readable code. A caller dispatches on this, never on
    /// the display string.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Parse { .. } => "expression_parse",
            Error::Unknown { .. } => "expression_unknown_variable",
            Error::Missing { .. } => "expression_missing_member",
            Error::OutOfRange { .. } => "expression_index_out_of_range",
            Error::Type { .. } => "expression_type",
            Error::Overflow { .. } => "expression_overflow",
            Error::DivideByZero { .. } => "expression_divide_by_zero",
            Error::Function { .. } => "expression_function",
            Error::UnknownMethod { .. } => "expression_unknown_method",
            Error::Convert { .. } => "expression_convert",
            Error::TooLarge { .. } => "expression_too_large",
            Error::TooManyTokens { .. } => "expression_too_many_tokens",
            Error::TooManyNodes { .. } => "expression_too_many_nodes",
            Error::TooDeep { .. } => "expression_too_deep",
            Error::TooManyArguments { .. } => "expression_too_many_arguments",
            Error::BudgetExceeded { .. } => "expression_budget_exceeded",
            Error::DeadlineExceeded => "expression_deadline_exceeded",
            Error::Cancelled => "expression_cancelled",
            Error::ValueLimit { .. } => "expression_value_limit",
            Error::FunctionPanicked { .. } => "expression_function_panicked",
            Error::FunctionRejected { .. } => "expression_function_rejected",
            Error::ResultType { .. } => "expression_result_type",
            Error::NonFinite { .. } => "expression_non_finite",
        }
    }

    pub(crate) fn parse(message: impl Into<String>, pos: usize) -> Error {
        Error::Parse {
            message: bounded(message.into()),
            pos,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { message, pos } => {
                write!(f, "parse error at byte {pos}: {}", Cut(message))
            }
            Error::Unknown { name } => write!(f, "unknown variable '{}'", Cut(name)),
            Error::Missing { name } => write!(f, "no such member '{}'", Cut(name)),
            Error::OutOfRange { index, len } => {
                write!(f, "index {index} is out of range for a list of {len}")
            }
            Error::Type { operation, got } => {
                write!(f, "no {operation} for a {got} value")
            }
            Error::Overflow { operation } => write!(f, "'{operation}' overflowed an int"),
            Error::DivideByZero { operation } => write!(f, "'{operation}' by zero"),
            Error::Function { name, message } => {
                write!(f, "'{}' failed: {}", Cut(name), Cut(message))
            }
            Error::UnknownMethod { name } => write!(f, "no such method '{}'", Cut(name)),
            Error::Convert { message } => write!(f, "{}", Cut(message)),
            Error::TooLarge { what, limit } => {
                write!(f, "the {what} exceeds the limit of {limit}")
            }
            Error::TooManyTokens { limit } => {
                write!(f, "the expression has more than {limit} tokens")
            }
            Error::TooManyNodes { limit } => {
                write!(f, "the expression has more than {limit} nodes")
            }
            Error::TooDeep { limit } => {
                write!(f, "the expression is deeper than {limit} levels")
            }
            Error::TooManyArguments { limit } => {
                write!(f, "a call has more than {limit} arguments")
            }
            Error::BudgetExceeded { kind, limit } => {
                write!(
                    f,
                    "the expression exceeded its {} budget of {limit}",
                    kind.as_str()
                )
            }
            Error::DeadlineExceeded => write!(f, "the expression deadline was exceeded"),
            Error::Cancelled => write!(f, "the expression was cancelled"),
            Error::ValueLimit { what, limit } => {
                write!(f, "a value's {what} exceeds the limit of {limit}")
            }
            Error::FunctionPanicked { name } => {
                write!(f, "'{}' panicked", Cut(name))
            }
            Error::FunctionRejected { name, reason } => {
                write!(f, "'{}' was rejected: {reason}", Cut(name))
            }
            Error::ResultType { expected, got } => {
                write!(f, "expected a {expected} result, got a {got} value")
            }
            Error::NonFinite { operation } => {
                write!(f, "'{operation}' produced a non-finite number")
            }
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `{:?}` reaches logs and `assert_eq!` diagnostics too, so use the
        // bounded display form rather than deriving a full-message dump.
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for Error {}

fn bounded(mut text: String) -> String {
    if text.len() <= MAX_ERROR_TEXT {
        return text;
    }
    let mut end = MAX_ERROR_TEXT - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('…');
    text
}

/// A borrowed display that cuts at a UTF-8 boundary and never prints more
/// than [`MAX_ERROR_TEXT`] bytes.
struct Cut<'a>(&'a str);

impl fmt::Display for Cut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.len() <= MAX_ERROR_TEXT {
            return f.write_str(self.0);
        }
        let mut end = MAX_ERROR_TEXT;
        while !self.0.is_char_boundary(end) {
            end -= 1;
        }
        f.write_str(&self.0[..end])?;
        f.write_str("…")
    }
}

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
            Error::TooLarge {
                what: "source",
                limit: 1,
            },
            Error::TooManyTokens { limit: 1 },
            Error::TooManyNodes { limit: 1 },
            Error::TooDeep { limit: 1 },
            Error::TooManyArguments { limit: 1 },
            Error::BudgetExceeded {
                kind: BudgetKind::Steps,
                limit: 1,
            },
            Error::DeadlineExceeded,
            Error::Cancelled,
            Error::ValueLimit {
                what: "depth",
                limit: 1,
            },
            Error::FunctionPanicked {
                name: "f".to_string(),
            },
            Error::FunctionRejected {
                name: "f".to_string(),
                reason: "effectful",
            },
            Error::ResultType {
                expected: "bool",
                got: "int",
            },
            Error::NonFinite { operation: "+" },
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
        assert_eq!(
            Error::TooLarge {
                what: "source",
                limit: 10
            }
            .to_string(),
            "the source exceeds the limit of 10"
        );
        assert_eq!(
            Error::BudgetExceeded {
                kind: BudgetKind::Steps,
                limit: 10
            }
            .to_string(),
            "the expression exceeded its step budget of 10"
        );
        assert_eq!(
            Error::ResultType {
                expected: "bool",
                got: "int"
            }
            .to_string(),
            "expected a bool result, got a int value"
        );
    }

    #[test]
    fn codes_are_stable_and_distinct() {
        assert_eq!(Error::parse("x", 0).code(), "expression_parse");
        assert_eq!(
            Error::DeadlineExceeded.code(),
            "expression_deadline_exceeded"
        );
        assert_eq!(
            Error::BudgetExceeded {
                kind: BudgetKind::Steps,
                limit: 1
            }
            .code(),
            "expression_budget_exceeded"
        );
    }

    #[test]
    fn long_text_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(MAX_ERROR_TEXT);
        let error = Error::parse(long.clone(), 0);
        let rendered = error.to_string();
        assert!(rendered.len() <= MAX_ERROR_TEXT + 32, "{}", rendered.len());
        assert!(rendered.ends_with('…'));

        let name = "x".repeat(MAX_ERROR_TEXT * 2);
        let rendered = Error::Unknown { name }.to_string();
        assert!(rendered.len() <= MAX_ERROR_TEXT + 32);
        assert!(rendered.contains('…'));
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

#[cfg(test)]
mod coverage_tests {
    use super::*;

    #[test]
    fn every_budget_kind_and_error_code_is_stable() {
        assert_eq!(BudgetKind::Steps.as_str(), "step");
        assert_eq!(BudgetKind::Allocation.as_str(), "allocation");
        assert_eq!(BudgetKind::Output.as_str(), "output");
        assert_eq!(BudgetKind::FunctionCalls.as_str(), "function-call");
        assert_eq!(BudgetKind::CollectionItems.as_str(), "collection-item");

        let cases = [
            (
                Error::parse("expected", 1),
                "expression_parse",
                "parse error at byte 1: expected",
            ),
            (
                Error::Unknown {
                    name: "x".to_string(),
                },
                "expression_unknown_variable",
                "unknown variable 'x'",
            ),
            (
                Error::Missing {
                    name: "x".to_string(),
                },
                "expression_missing_member",
                "no such member 'x'",
            ),
            (
                Error::OutOfRange { index: 2, len: 1 },
                "expression_index_out_of_range",
                "index 2 is out of range for a list of 1",
            ),
            (
                Error::Type {
                    operation: "+",
                    got: "bool",
                },
                "expression_type",
                "no + for a bool value",
            ),
            (
                Error::Overflow { operation: "*" },
                "expression_overflow",
                "'*' overflowed an int",
            ),
            (
                Error::DivideByZero { operation: "/" },
                "expression_divide_by_zero",
                "'/' by zero",
            ),
            (
                Error::Function {
                    name: "f".to_string(),
                    message: "bad".to_string(),
                },
                "expression_function",
                "'f' failed: bad",
            ),
            (
                Error::UnknownMethod {
                    name: "m".to_string(),
                },
                "expression_unknown_method",
                "no such method 'm'",
            ),
            (
                Error::Convert {
                    message: "bad".to_string(),
                },
                "expression_convert",
                "bad",
            ),
            (
                Error::TooLarge {
                    what: "source",
                    limit: 1,
                },
                "expression_too_large",
                "the source exceeds the limit of 1",
            ),
            (
                Error::TooManyTokens { limit: 1 },
                "expression_too_many_tokens",
                "the expression has more than 1 tokens",
            ),
            (
                Error::TooManyNodes { limit: 1 },
                "expression_too_many_nodes",
                "the expression has more than 1 nodes",
            ),
            (
                Error::TooDeep { limit: 1 },
                "expression_too_deep",
                "the expression is deeper than 1 levels",
            ),
            (
                Error::TooManyArguments { limit: 1 },
                "expression_too_many_arguments",
                "a call has more than 1 arguments",
            ),
            (
                Error::BudgetExceeded {
                    kind: BudgetKind::Output,
                    limit: 1,
                },
                "expression_budget_exceeded",
                "the expression exceeded its output budget of 1",
            ),
            (
                Error::DeadlineExceeded,
                "expression_deadline_exceeded",
                "the expression deadline was exceeded",
            ),
            (
                Error::Cancelled,
                "expression_cancelled",
                "the expression was cancelled",
            ),
            (
                Error::ValueLimit {
                    what: "depth",
                    limit: 1,
                },
                "expression_value_limit",
                "a value's depth exceeds the limit of 1",
            ),
            (
                Error::FunctionPanicked {
                    name: "f".to_string(),
                },
                "expression_function_panicked",
                "'f' panicked",
            ),
            (
                Error::FunctionRejected {
                    name: "f".to_string(),
                    reason: "effectful",
                },
                "expression_function_rejected",
                "'f' was rejected: effectful",
            ),
            (
                Error::ResultType {
                    expected: "bool",
                    got: "int",
                },
                "expression_result_type",
                "expected a bool result, got a int value",
            ),
            (
                Error::NonFinite { operation: "+" },
                "expression_non_finite",
                "'+' produced a non-finite number",
            ),
        ];

        for (error, code, display) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), display);
            assert_eq!(format!("{error:?}"), display);
        }
    }
}
