//! The interpreter, and the injection surface it evaluates against.

use crate::parser::{BinOp, Node};
use crate::value::Value;
use crate::{Error, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// The variables and methods one evaluation sees.
///
/// Variables are plain values — scalars, lists and objects ([`Value::Map`]) —
/// and functions are [`Value::Function`]s, registered under the name they are
/// called by. A function is called with its arguments as `f(x)`, or as a
/// method with its receiver first, `x.f()`; a function stored *inside* an
/// object is called that way through the object (`step1.normalize()`), which
/// is how an injected object carries its own methods.
#[derive(Debug, Clone, Default)]
pub struct Context {
    values: HashMap<Arc<str>, Value>,
}

impl Context {
    pub fn new() -> Context {
        Context::default()
    }

    /// Bind a name, replacing whatever it held. Chainable.
    pub fn set(&mut self, name: impl Into<Arc<str>>, value: impl Into<Value>) -> &mut Context {
        self.values.insert(name.into(), value.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.values.remove(name)
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The bound names, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(AsRef::as_ref)
    }
}

impl<'a> FromIterator<(&'a str, Value)> for Context {
    fn from_iter<T: IntoIterator<Item = (&'a str, Value)>>(iter: T) -> Self {
        Context {
            values: iter
                .into_iter()
                .map(|(name, value)| (Arc::from(name), value))
                .collect(),
        }
    }
}

pub(crate) fn eval(nodes: &[Node], root: u32, context: &Context) -> Result<Value> {
    eval_node(nodes, root, context)
}

fn eval_node(nodes: &[Node], index: u32, context: &Context) -> Result<Value> {
    match &nodes[index as usize] {
        Node::Null => Ok(Value::Null),
        Node::Bool(b) => Ok(Value::Bool(*b)),
        Node::Int(i) => Ok(Value::Int(*i)),
        Node::Float(f) => Ok(Value::Float(*f)),
        Node::Str(s) => Ok(Value::Str(s.clone())),
        Node::Ident(name) => context.get(name).cloned().ok_or_else(|| Error::Unknown {
            name: name.to_string(),
        }),
        Node::Member(target, name) => {
            let value = eval_node(nodes, *target, context)?;
            let map = value.as_map().ok_or_else(|| type_error(".", &value))?;

            map.get(name.as_ref())
                .cloned()
                .ok_or_else(|| Error::Missing {
                    name: name.to_string(),
                })
        }
        Node::Index(target, key) => {
            let container = eval_node(nodes, *target, context)?;
            let key = eval_node(nodes, *key, context)?;

            match (&container, &key) {
                (Value::List(items), Value::Int(i)) => {
                    let len = items.len();
                    let position = usize::try_from(*i)
                        .ok()
                        .filter(|position| *position < len)
                        .ok_or(Error::OutOfRange { index: *i, len })?;

                    Ok(items[position].clone())
                }
                (Value::Map(map), Value::Str(name)) => {
                    map.get(name.as_ref())
                        .cloned()
                        .ok_or_else(|| Error::Missing {
                            name: name.to_string(),
                        })
                }
                _ => Err(type_error("[]", &container)),
            }
        }
        Node::Call(name, args) => {
            let function = context.get(name).cloned().ok_or_else(|| Error::Unknown {
                name: name.to_string(),
            })?;
            let Value::Function(function) = function else {
                return Err(type_error("call", &function));
            };

            function(&arguments(nodes, args, context)?)
        }
        Node::Method(receiver, name, args) => {
            let receiver = eval_node(nodes, *receiver, context)?;

            // The object's own method first — that is what makes an injected
            // object carry its methods — then a method of that name from the
            // context, which is the `size(x)` / `x.size()` pairing, and last
            // the built-in methods a value type carries of its own
            // (`name.length()`, `items.contains(x)`).
            let own = receiver
                .as_map()
                .and_then(|map| map.get(name.as_ref()))
                .filter(|value| value.is_function())
                .cloned();

            let function = match own {
                Some(function) => function,
                None => match context.get(name).cloned() {
                    Some(function) => function,
                    None => {
                        let args = arguments(nodes, args, context)?;

                        return crate::methods::call(name, &receiver, &args)?.ok_or_else(|| {
                            Error::UnknownMethod {
                                name: name.to_string(),
                            }
                        });
                    }
                },
            };
            let Value::Function(function) = function else {
                return Err(Error::UnknownMethod {
                    name: name.to_string(),
                });
            };

            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(receiver);
            call_args.extend(arguments(nodes, args, context)?);

            function(&call_args)
        }
        Node::Not(operand) => {
            let value = eval_node(nodes, *operand, context)?;
            let boolean = boolean_of(&value, "!")?;

            Ok(Value::Bool(!boolean))
        }
        Node::Neg(operand) => {
            let value = eval_node(nodes, *operand, context)?;

            match value {
                Value::Int(i) => i
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or(Error::Overflow { operation: "-" }),
                Value::Float(f) => Ok(Value::Float(-f)),
                other => Err(type_error("-", &other)),
            }
        }
        Node::Binary(op, left, right) => binary(nodes, *op, *left, *right, context),
    }
}

/// Evaluate `&&` and `||` with their short circuit, and everything else by
/// evaluating both sides.
fn binary(nodes: &[Node], op: BinOp, left: u32, right: u32, context: &Context) -> Result<Value> {
    match op {
        BinOp::And => {
            let left = eval_node(nodes, left, context)?;
            if !boolean_of(&left, op.symbol())? {
                return Ok(Value::Bool(false));
            }
            let right = eval_node(nodes, right, context)?;

            return Ok(Value::Bool(boolean_of(&right, op.symbol())?));
        }
        BinOp::Or => {
            let left = eval_node(nodes, left, context)?;
            if boolean_of(&left, op.symbol())? {
                return Ok(Value::Bool(true));
            }
            let right = eval_node(nodes, right, context)?;

            return Ok(Value::Bool(boolean_of(&right, op.symbol())?));
        }
        _ => {}
    }

    let left = eval_node(nodes, left, context)?;
    let right = eval_node(nodes, right, context)?;

    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            arithmetic(op, &left, &right)
        }
        BinOp::Eq => Ok(Value::Bool(left.equals(&right))),
        BinOp::Ne => Ok(Value::Bool(!left.equals(&right))),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ordering = left
                .compare(&right)
                .ok_or_else(|| type_error(op.symbol(), blame(&left, &right)))?;

            Ok(Value::Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinOp::And | BinOp::Or => unreachable!("handled above"),
    }
}

/// `+`, `-`, `*`, `/` and `%`. Two ints stay an int and keep CEL's semantics
/// (`/` truncates, `%` is the remainder, a result past `i64` overflows); a
/// `uint` on either side is computed in 128 bits, exact, and narrowed back to
/// whichever integer kind holds the result; a float either side makes it a
/// float. The operator that reads in the source is the one the error names.
fn arithmetic(op: BinOp, left: &Value, right: &Value) -> Result<Value> {
    let symbol = op.symbol();

    // Two ints stay an int, and a result past `i64` overflows rather than
    // quietly becoming a wider type.
    if let (Value::Int(a), Value::Int(b)) = (left, right) {
        let (a, b) = (*a, *b);

        let result = match op {
            BinOp::Add => a.checked_add(b),
            BinOp::Sub => a.checked_sub(b),
            BinOp::Mul => a.checked_mul(b),
            BinOp::Div | BinOp::Rem => {
                if b == 0 {
                    return Err(Error::DivideByZero { operation: symbol });
                }
                if op == BinOp::Div {
                    a.checked_div(b)
                } else {
                    a.checked_rem(b)
                }
            }
            _ => unreachable!("only arithmetic reaches here"),
        };

        return result
            .map(Value::Int)
            .ok_or(Error::Overflow { operation: symbol });
    }

    // A `uint` on either side: computed in 128 bits, exact, and narrowed back
    // to the kind that holds the result.
    if let (Some(a), Some(b)) = (left.as_i128(), right.as_i128()) {
        let result = match op {
            BinOp::Add => a.checked_add(b),
            BinOp::Sub => a.checked_sub(b),
            BinOp::Mul => a.checked_mul(b),
            BinOp::Div | BinOp::Rem => {
                if b == 0 {
                    return Err(Error::DivideByZero { operation: symbol });
                }
                if op == BinOp::Div {
                    a.checked_div(b)
                } else {
                    a.checked_rem(b)
                }
            }
            _ => unreachable!("only arithmetic reaches here"),
        };

        let result = result.ok_or(Error::Overflow { operation: symbol })?;

        return match (i64::try_from(result), u64::try_from(result)) {
            (Ok(value), _) => Ok(Value::Int(value)),
            (Err(_), Ok(value)) => Ok(Value::UInt(value)),
            (Err(_), Err(_)) => Err(Error::Overflow { operation: symbol }),
        };
    }

    let (Some(a), Some(b)) = (left.as_number(), right.as_number()) else {
        // `+` is the one arithmetic operator strings define.
        if op == BinOp::Add
            && let (Value::Str(a), Value::Str(b)) = (left, right)
        {
            let mut text = String::with_capacity(a.len() + b.len());
            text.push_str(a);
            text.push_str(b);

            return Ok(Value::Str(Arc::from(text)));
        }

        return Err(type_error(symbol, blame(left, right)));
    };

    if matches!(op, BinOp::Div | BinOp::Rem) && b == 0.0 {
        return Err(Error::DivideByZero { operation: symbol });
    }

    Ok(Value::Float(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        _ => unreachable!("only arithmetic reaches here"),
    }))
}

fn arguments(nodes: &[Node], args: &[u32], context: &Context) -> Result<Vec<Value>> {
    args.iter()
        .map(|arg| eval_node(nodes, *arg, context))
        .collect()
}

fn boolean_of(value: &Value, operation: &'static str) -> Result<bool> {
    value.as_bool().ok_or_else(|| type_error(operation, value))
}

/// The operand an error message blames: the one whose type has no overload,
/// which is the right side whenever the left one is a number or a string.
fn blame<'a>(left: &'a Value, right: &'a Value) -> &'a Value {
    if left.is_number() || left.as_str().is_some() {
        right
    } else {
        left
    }
}

fn type_error(operation: &'static str, value: &Value) -> Error {
    Error::Type {
        operation,
        got: value.type_name(),
    }
}
