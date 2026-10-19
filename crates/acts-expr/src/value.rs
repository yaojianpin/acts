//! The value type an expression reads and produces, and the injection surface:
//! objects are [`Map`]s, methods are [`Value::Function`]s.

use crate::{Error, Result};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// An object: a string-keyed map of values. Usually behind an [`Arc`], so
/// reading a member is a pointer clone.
pub type Map = HashMap<String, Value>;

/// A native method. The receiver of a method call is its first argument, so
/// `name.upper()` and `upper(name)` are the same call.
pub type NativeFunction = dyn Fn(&[Value]) -> Result<Value> + Send + Sync;

/// One expression value.
///
/// Values are immutable and cheap to clone: strings, lists, maps and functions
/// are behind an [`Arc`].
///
/// `PartialEq` is the expression language's `==` (see [`Value::equals`]), so a
/// host comparing values in Rust compares them the way the workflow does.
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// A 64-bit integer: what an integer literal is, and what `+`, `-`, `*`,
    /// `/` and `%` stay in while both operands are ints.
    Int(i64),
    /// An integer above `i64::MAX` — what JSON gives a host for one, since a
    /// JSON integer is an `i64` or a `u64` and nothing else represents
    /// `u64::MAX` exactly. A value this wide is never produced from a literal
    /// and only ever shows up injected; arithmetic that mixes it with an `int`
    /// is exact.
    UInt(u64),
    /// A 64-bit float. Mixed int/float arithmetic produces a float.
    Float(f64),
    Str(Arc<str>),
    List(Arc<[Value]>),
    /// An injected object. `a.b` reads a member, `a['b']` the same key.
    Map(Arc<Map>),
    /// An injected method. `f(x)` calls it directly, `a.f(x)` calls it with
    /// `a` as its first argument, and a function stored *in* a map is called
    /// the same way when reached through that map.
    Function(Arc<NativeFunction>),
}

impl Value {
    /// Wrap a closure as an injectable method.
    ///
    /// ```rust
    /// use acts_expr::Value;
    ///
    /// let upper = Value::function(|args| match args {
    ///     [Value::Str(s)] => Ok(Value::from(s.to_uppercase())),
    ///     _ => Err(acts_expr::Error::Type { operation: "upper", got: "other" }),
    /// });
    /// ```
    pub fn function<F>(f: F) -> Value
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        Value::Function(Arc::new(f))
    }

    /// Build an object from key/value pairs.
    ///
    /// ```rust
    /// use acts_expr::Value;
    ///
    /// let step = Value::map([("id", Value::from("step1")), ("retries", Value::from(2))]);
    /// ```
    pub fn map<K, V, I>(entries: I) -> Value
    where
        K: Into<String>,
        V: Into<Value>,
        I: IntoIterator<Item = (K, V)>,
    {
        Value::Map(Arc::new(
            entries
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        ))
    }

    /// Build a list.
    pub fn list<I, V>(items: I) -> Value
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        Value::List(items.into_iter().map(Into::into).collect())
    }

    /// The type name the errors and `Display` use.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::UInt(_) => "uint",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Function(_) => "function",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Whether the value is an `int`, a `uint` or a `float`.
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::UInt(_) | Value::Float(_))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The value as a `u64`, for an `int` that is not negative too.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Int(i) => u64::try_from(*i).ok(),
            Value::UInt(u) => Some(*u),
            _ => None,
        }
    }

    /// The value as a 128-bit integer: exact for both integer kinds, which is
    /// what lets `int` and `uint` mix without a rounding step.
    pub fn as_i128(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(i128::from(*i)),
            Value::UInt(u) => Some(i128::from(*u)),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The number as an `f64`, whatever kind of number it is.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::UInt(u) => Some(*u as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Value::Map(map) => Some(map),
            _ => None,
        }
    }

    pub fn is_function(&self) -> bool {
        matches!(self, Value::Function(_))
    }

    /// Value equality, as `==` defines it: numbers compare across `int` and
    /// `float`, `null` is only equal to `null`, lists and maps compare
    /// element-wise, and values of different kinds are simply not equal.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::UInt(a), Value::UInt(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            // Exact across the integer kinds, in the one width both fit.
            (Value::Int(a), Value::UInt(b)) | (Value::UInt(b), Value::Int(a)) => {
                i128::from(*a) == i128::from(*b)
            }
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                (*a as f64) == *b
            }
            (Value::UInt(a), Value::Float(b)) | (Value::Float(b), Value::UInt(a)) => {
                (*a as f64) == *b
            }
            // Immutable and usually shared, so the pointer check answers the
            // common case (a value compared with itself) without walking.
            (Value::List(a), Value::List(b)) => {
                std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
                    || (a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y)))
            }
            (Value::Map(a), Value::Map(b)) => {
                std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
                    || (a.len() == b.len()
                        && a.iter()
                            .all(|(k, v)| b.get(k).is_some_and(|other| v.equals(other))))
            }
            (Value::Function(a), Value::Function(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Ordering for `<`, `<=`, `>` and `>=`: numbers across `int` and `float`,
    /// and strings by their bytes. `None` for every other pair, which the
    /// evaluator reports as a type error.
    pub fn compare(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::UInt(a), Value::UInt(b)) => Some(a.cmp(b)),
            (Value::Int(a), Value::UInt(b)) => i128::from(*a).partial_cmp(&i128::from(*b)),
            (Value::UInt(a), Value::Int(b)) => i128::from(*a).partial_cmp(&i128::from(*b)),
            (Value::Str(a), Value::Str(b)) => Some(a.as_ref().cmp(b.as_ref())),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::UInt(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Float(a), Value::UInt(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            _ => None,
        }
    }

    /// A JSON value for this value. Functions have no JSON form and are an
    /// error; a non-finite float is too.
    #[cfg(feature = "json")]
    pub fn to_json(&self) -> Result<serde_json::Value> {
        use serde_json::Value as Json;

        Ok(match self {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Int(i) => Json::from(*i),
            Value::UInt(u) => Json::from(*u),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(Json::Number)
                .ok_or_else(|| Error::Convert {
                    message: format!("{f} has no JSON representation"),
                })?,
            Value::Str(s) => Json::String(s.to_string()),
            Value::List(items) => {
                Json::Array(items.iter().map(Value::to_json).collect::<Result<_>>()?)
            }
            Value::Map(map) => Json::Object(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), v.to_json()?)))
                    .collect::<Result<_>>()?,
            ),
            Value::Function(_) => {
                return Err(Error::Convert {
                    message: "a function has no JSON representation".to_string(),
                });
            }
        })
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::UInt(u) => write!(f, "{u}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::List(items) => f.debug_list().entries(items.iter()).finish(),
            Value::Map(map) => f.debug_map().entries(map.iter()).finish(),
            Value::Function(_) => f.write_str("<function>"),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.equals(other)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::UInt(u) => write!(f, "{u}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Value::Map(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::Function(_) => write!(f, "<function>"),
        }
    }
}

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Value::Null
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(value as i64)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::Int(i64::from(value))
    }
}

impl From<u64> for Value {
    /// A `u64` that fits an `i64` becomes one, so a `uint` only ever holds a
    /// value that needs the wider type.
    fn from(value: u64) -> Self {
        match i64::try_from(value) {
            Ok(value) => Value::Int(value),
            Err(_) => Value::UInt(value),
        }
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::from(value as u64)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(Arc::from(value))
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(Arc::from(value))
    }
}

impl From<Arc<str>> for Value {
    fn from(value: Arc<str>) -> Self {
        Value::Str(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::List(value.into())
    }
}

impl From<Map> for Value {
    fn from(value: Map) -> Self {
        Value::Map(Arc::new(value))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Null, Into::into)
    }
}

#[cfg(feature = "json")]
impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        use serde_json::Value as Json;

        match value {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(b),
            // The same ladder `serde_json` reads its own numbers with: an
            // `i64`, then a `u64`, and a float only past both.
            Json::Number(n) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Value::Int(i),
                (None, Some(u)) => Value::UInt(u),
                (None, None) => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            Json::String(s) => Value::Str(Arc::from(s)),
            Json::Array(items) => Value::List(items.into_iter().map(Value::from).collect()),
            Json::Object(map) => Value::Map(Arc::new(
                map.into_iter().map(|(k, v)| (k, Value::from(v))).collect(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn constructors_build_the_kinds() {
        assert_eq!(Value::from(1), Value::Int(1));
        assert_eq!(Value::from(1.5), Value::Float(1.5));
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from("text"), Value::from("text"));
        assert_eq!(Value::from("text"), Value::Str(Arc::from("text")));
        assert_eq!(
            Value::from(vec![Value::from(1)]),
            Value::List(Arc::from(vec![Value::from(1)]))
        );
        assert_eq!(
            Value::map([("k", 1)]),
            Value::Map(Arc::new(HashMap::from([("k".to_string(), Value::Int(1))])))
        );
        assert_eq!(
            Value::list([1, 2]),
            Value::List(Arc::from(vec![Value::Int(1), Value::Int(2)]))
        );
        assert!(Value::default().is_null());
    }

    #[test]
    fn from_conversions_cover_each_width() {
        assert_eq!(Value::from(()), Value::Null);
        assert_eq!(Value::from(false), Value::Bool(false));
        assert_eq!(Value::from(1i32), Value::Int(1));
        assert_eq!(Value::from(-1i64), Value::Int(-1));
        assert_eq!(Value::from(1u32), Value::Int(1));
        assert_eq!(Value::from(1u64), Value::Int(1));
        assert_eq!(Value::from(u64::MAX), Value::UInt(u64::MAX));
        assert_eq!(Value::from(7usize), Value::Int(7));
        assert_eq!(Value::from(usize::MAX), Value::from(usize::MAX as u64));
        assert_eq!(Value::from(2.5f64), Value::Float(2.5));
        assert_eq!(Value::from(String::from("s")), Value::Str(Arc::from("s")));
        assert_eq!(
            Value::from(Arc::<str>::from("s")),
            Value::Str(Arc::from("s"))
        );
        assert_eq!(Value::from(Some(3)), Value::Int(3));
        assert_eq!(Value::from(None::<i32>), Value::Null);

        let map: Map = HashMap::from([("k".to_string(), Value::Int(1))]);
        assert_eq!(Value::from(map), Value::map([("k", 1)]));
    }

    #[test]
    fn type_names_and_kind_predicates() {
        let cases = [
            (Value::Null, "null"),
            (Value::Bool(true), "bool"),
            (Value::Int(1), "int"),
            (Value::UInt(1), "uint"),
            (Value::Float(1.0), "float"),
            (Value::from("s"), "string"),
            (Value::list([1]), "list"),
            (Value::map([("k", 1)]), "map"),
            (Value::function(|_| Ok(Value::Null)), "function"),
        ];

        for (value, name) in cases {
            assert_eq!(value.type_name(), name);
            assert_eq!(value.is_null(), name == "null");
            assert_eq!(value.is_number(), matches!(name, "int" | "uint" | "float"));
            assert_eq!(value.is_function(), name == "function");
        }
    }

    #[test]
    fn accessors_answer_only_for_their_kind() {
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Int(1).as_bool(), None);

        assert_eq!(Value::Int(-2).as_int(), Some(-2));
        assert_eq!(Value::UInt(2).as_int(), None);

        assert_eq!(Value::Int(2).as_u64(), Some(2));
        assert_eq!(Value::Int(-2).as_u64(), None);
        assert_eq!(Value::UInt(u64::MAX).as_u64(), Some(u64::MAX));
        assert_eq!(Value::Float(1.0).as_u64(), None);

        assert_eq!(Value::Int(-2).as_i128(), Some(-2));
        assert_eq!(Value::UInt(u64::MAX).as_i128(), Some(i128::from(u64::MAX)));
        assert_eq!(Value::Bool(true).as_i128(), None);

        assert_eq!(Value::Float(1.5).as_float(), Some(1.5));
        assert_eq!(Value::Int(1).as_float(), None);

        assert_eq!(Value::Int(1).as_number(), Some(1.0));
        assert_eq!(Value::UInt(2).as_number(), Some(2.0));
        assert_eq!(Value::Float(3.5).as_number(), Some(3.5));
        assert_eq!(Value::from("1").as_number(), None);

        assert_eq!(Value::from("s").as_str(), Some("s"));
        assert_eq!(Value::Int(1).as_str(), None);

        assert_eq!(
            Value::list([1, 2]).as_list(),
            Some(&[Value::Int(1), Value::Int(2)][..])
        );
        assert_eq!(Value::Int(1).as_list(), None);

        assert_eq!(Value::map([("k", 1)]).as_map().unwrap().len(), 1);
        assert_eq!(Value::Int(1).as_map(), None);
    }

    #[test]
    fn equality_follows_the_expression_language() {
        assert!(Value::Null.equals(&Value::Null));
        assert!(!Value::Null.equals(&Value::Int(0)));
        assert!(Value::Bool(true).equals(&Value::Bool(true)));
        assert!(!Value::Bool(true).equals(&Value::Bool(false)));
        assert!(Value::from("a").equals(&Value::from("a")));
        assert!(!Value::from("a").equals(&Value::from("b")));

        assert!(Value::Int(1).equals(&Value::Int(1)));
        assert!(Value::UInt(1).equals(&Value::UInt(1)));
        assert!(Value::Float(1.5).equals(&Value::Float(1.5)));
        assert!(Value::Int(1).equals(&Value::UInt(1)));
        assert!(Value::UInt(1).equals(&Value::Int(1)));
        assert!(Value::Int(1).equals(&Value::Float(1.0)));
        assert!(Value::Float(1.0).equals(&Value::Int(1)));
        assert!(Value::UInt(1).equals(&Value::Float(1.0)));
        assert!(Value::Float(1.0).equals(&Value::UInt(1)));
        assert!(!Value::Int(1).equals(&Value::UInt(2)));

        // A shared list or map answers by pointer, a rebuilt one by content.
        let list = Value::list([1, 2]);
        assert!(list.equals(&list));
        assert!(list.equals(&Value::list([1, 2])));
        assert!(!list.equals(&Value::list([1, 2, 3])));
        assert!(!list.equals(&Value::list([1, 3])));

        let map = Value::map([("a", 1), ("b", 2)]);
        assert!(map.equals(&map));
        assert!(map.equals(&Value::map([("a", 1), ("b", 2)])));
        assert!(!map.equals(&Value::map([("a", 1)])));
        assert!(!map.equals(&Value::map([("a", 1), ("b", 3)])));

        let function = Value::function(|_| Ok(Value::Null));
        assert!(function.equals(&function));
        assert!(!function.equals(&Value::function(|_| Ok(Value::Null))));
        assert!(!function.equals(&Value::Null));

        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_ne!(Value::Int(1), Value::UInt(2));
    }

    #[test]
    fn ordering_covers_both_number_kinds_and_strings() {
        use std::cmp::Ordering::{Equal, Greater, Less};

        assert_eq!(Value::Int(1).compare(&Value::Int(2)), Some(Less));
        assert_eq!(Value::UInt(2).compare(&Value::UInt(2)), Some(Equal));
        assert_eq!(Value::Int(-1).compare(&Value::UInt(1)), Some(Less));
        assert_eq!(
            Value::UInt(u64::MAX).compare(&Value::Int(-1)),
            Some(Greater)
        );
        assert_eq!(Value::from("a").compare(&Value::from("b")), Some(Less));
        assert_eq!(Value::Int(1).compare(&Value::Float(1.5)), Some(Less));
        assert_eq!(Value::UInt(2).compare(&Value::Float(1.5)), Some(Greater));
        assert_eq!(Value::Float(1.5).compare(&Value::Int(1)), Some(Greater));
        assert_eq!(Value::Float(1.5).compare(&Value::UInt(2)), Some(Less));
        assert_eq!(Value::Float(1.0).compare(&Value::Float(2.0)), Some(Less));
        assert_eq!(Value::Bool(true).compare(&Value::Bool(true)), None);
        assert_eq!(Value::Null.compare(&Value::Null), None);
    }

    #[test]
    fn display_and_debug_render_each_kind() {
        assert_eq!(Value::Null.to_string(), "null");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Int(-1).to_string(), "-1");
        assert_eq!(Value::UInt(u64::MAX).to_string(), u64::MAX.to_string());
        assert_eq!(Value::Float(1.5).to_string(), "1.5");
        assert_eq!(Value::from("a\nb").to_string(), "a\nb");
        assert_eq!(
            Value::list([Value::from(1), Value::from("x")]).to_string(),
            "[1, x]"
        );
        assert_eq!(Value::map([("k", 1)]).to_string(), "{k: 1}");
        assert_eq!(
            Value::function(|_| Ok(Value::Null)).to_string(),
            "<function>"
        );

        assert_eq!(format!("{:?}", Value::Null), "null");
        assert_eq!(format!("{:?}", Value::Bool(false)), "false");
        assert_eq!(format!("{:?}", Value::Int(2)), "2");
        assert_eq!(format!("{:?}", Value::UInt(2)), "2");
        assert_eq!(format!("{:?}", Value::Float(1.5)), "1.5");
        assert_eq!(format!("{:?}", Value::from("a\nb")), "\"a\\nb\"");
        assert_eq!(format!("{:?}", Value::list([1, 2])), "[1, 2]");
        assert_eq!(format!("{:?}", Value::map([("k", 1)])), "{\"k\": 1}");
        assert_eq!(
            format!("{:?}", Value::function(|_| Ok(Value::Null))),
            "<function>"
        );

        assert_eq!(
            format!("{}", Value::list(std::iter::empty::<Value>())),
            "[]"
        );
        assert_eq!(format!("{}", Value::map(Map::new())), "{}");
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_conversion_covers_every_kind() {
        use serde_json::json;

        assert_eq!(Value::from(json!(null)), Value::Null);
        assert_eq!(Value::from(json!(true)), Value::Bool(true));
        assert_eq!(Value::from(json!(-3)), Value::Int(-3));
        assert_eq!(Value::from(json!(u64::MAX)), Value::UInt(u64::MAX));
        assert_eq!(Value::from(json!(1.5)), Value::Float(1.5));
        assert_eq!(Value::from(json!("s")), Value::from("s"));
        assert_eq!(
            Value::from(json!([1, "two"])),
            Value::list([Value::from(1), Value::from("two")])
        );
        assert_eq!(Value::from(json!({"k": false})), Value::map([("k", false)]));

        assert_eq!(Value::Null.to_json().unwrap(), json!(null));
        assert_eq!(Value::Bool(true).to_json().unwrap(), json!(true));
        assert_eq!(Value::Int(1).to_json().unwrap(), json!(1));
        assert_eq!(Value::UInt(u64::MAX).to_json().unwrap(), json!(u64::MAX));
        assert_eq!(Value::Float(1.5).to_json().unwrap(), json!(1.5));
        assert_eq!(Value::from("s").to_json().unwrap(), json!("s"));
        assert_eq!(
            Value::list([Value::from(1), Value::from("two")])
                .to_json()
                .unwrap(),
            json!([1, "two"])
        );
        assert_eq!(
            Value::map([("k", false)]).to_json().unwrap(),
            json!({"k": false})
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_rejects_functions_and_non_finite_floats() {
        let function = Value::function(|_| Ok(Value::Null));
        assert_eq!(
            function.to_json().unwrap_err(),
            Error::Convert {
                message: "a function has no JSON representation".to_string()
            }
        );
        assert_eq!(
            Value::from(f64::NAN).to_json().unwrap_err(),
            Error::Convert {
                message: "NaN has no JSON representation".to_string()
            }
        );
        assert_eq!(
            Value::from(f64::INFINITY).to_json().unwrap_err(),
            Error::Convert {
                message: "inf has no JSON representation".to_string()
            }
        );

        // A nested function fails the enclosing list or map too.
        assert!(
            Value::list([Value::from(1), function.clone()])
                .to_json()
                .is_err()
        );
        assert!(Value::map([("f", function)]).to_json().is_err());
    }
}
