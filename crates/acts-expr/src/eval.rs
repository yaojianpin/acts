//! The interpreter, and the injection surface it evaluates against.

use crate::parser::{BinOp, Node};
use crate::regex_pool::RegexPool;
use crate::value::{Value, ValueMeter};
use crate::{BudgetKind, Error, FunctionPurity, FunctionSpec, Result, ValueLimits};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A cancellation flag shared with the caller. Cancelling is advisory: the
/// evaluator observes it at every 64th step and at function boundaries.
#[derive(Debug, Clone, Default)]
pub struct Cancellation {
    flag: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

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
    specs: HashMap<Arc<str>, FunctionSpec>,
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

    /// Attach the contract of a function bound under `name`.
    ///
    /// The evaluator uses the default [`FunctionSpec`] for a name without one
    /// and for a function reached through an object.
    pub fn set_spec(&mut self, name: impl Into<Arc<str>>, spec: FunctionSpec) -> &mut Context {
        self.specs.insert(name.into(), spec);
        self
    }

    pub(crate) fn spec(&self, name: &str) -> FunctionSpec {
        self.specs.get(name).copied().unwrap_or_default()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.specs.remove(name);
        self.values.remove(name)
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.specs.clear();
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

    /// Check every injected value against `limits` before evaluation.
    ///
    /// A context is host data: process variables, snapshots and function
    /// results can be large or deeply nested even when the source is tiny.
    pub fn validate(&self, limits: &ValueLimits) -> Result<()> {
        let mut meter = ValueMeter::rejecting_non_finite(limits);
        for value in self.values.values() {
            meter.measure(value)?;
        }
        Ok(())
    }
}

impl<'a> FromIterator<(&'a str, Value)> for Context {
    fn from_iter<T: IntoIterator<Item = (&'a str, Value)>>(iter: T) -> Self {
        Context {
            values: iter
                .into_iter()
                .map(|(name, value)| (Arc::from(name), value))
                .collect(),
            specs: HashMap::new(),
        }
    }
}

/// The runtime budget one evaluation may spend.
///
/// It is consumed, not replaced: a caller that evaluates a query filter over
/// many rows passes the same budget to each row for a total budget. Every
/// field is checked before the corresponding work, so a budget error is
/// returned before an allocation or a collection walk grows.
#[derive(Debug)]
pub struct EvalBudget {
    limits: crate::EvalLimits,
    steps_left: u64,
    allocation_bytes_left: usize,
    output_bytes_left: usize,
    function_calls_left: u32,
    collection_items_left: u64,
    deadline: Option<Instant>,
    cancellation: Cancellation,
    ticks: u32,
}

impl EvalBudget {
    pub fn for_profile(profile: crate::BudgetProfile) -> EvalBudget {
        EvalBudget::with_limits(profile.eval_limits())
    }

    pub fn with_limits(limits: crate::EvalLimits) -> EvalBudget {
        let now = Instant::now();
        let deadline = if limits.deadline.is_zero() {
            Some(now)
        } else {
            now.checked_add(limits.deadline)
        };

        EvalBudget {
            limits,
            steps_left: limits.steps,
            allocation_bytes_left: limits.allocation_bytes,
            output_bytes_left: limits.output_bytes,
            function_calls_left: limits.function_calls,
            collection_items_left: limits.collection_items,
            deadline,
            cancellation: Cancellation::default(),
            ticks: 0,
        }
    }

    /// The value limits a context and results are checked against.
    pub fn value_limits(&self) -> &ValueLimits {
        &self.limits.value
    }

    pub fn remaining_steps(&self) -> u64 {
        self.steps_left
    }

    pub fn remaining_output_bytes(&self) -> usize {
        self.output_bytes_left
    }

    pub fn remaining_function_calls(&self) -> u32 {
        self.function_calls_left
    }

    /// A handle that cancels this evaluation from another thread.
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    /// Allow or forbid [`FunctionPurity::Effectful`] functions.
    pub fn set_allow_effectful_functions(&mut self, allow: bool) {
        self.limits.allow_effectful_functions = allow;
    }

    fn allow_effectful_functions(&self) -> bool {
        self.limits.allow_effectful_functions
    }

    pub(crate) fn step(&mut self, steps: u64) -> Result<()> {
        let Some(left) = self.steps_left.checked_sub(steps) else {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::Steps,
                limit: self.limits.steps,
            });
        };
        self.steps_left = left;
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(64) {
            self.check_time()?;
        }
        Ok(())
    }

    pub(crate) fn charge_allocation(&mut self, bytes: usize) -> Result<()> {
        let Some(left) = self.allocation_bytes_left.checked_sub(bytes) else {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::Allocation,
                limit: self.limits.allocation_bytes as u64,
            });
        };
        self.allocation_bytes_left = left;
        Ok(())
    }

    pub(crate) fn charge_output(&mut self, bytes: usize) -> Result<()> {
        let Some(left) = self.output_bytes_left.checked_sub(bytes) else {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::Output,
                limit: self.limits.output_bytes as u64,
            });
        };
        self.output_bytes_left = left;
        Ok(())
    }

    pub(crate) fn charge_items(&mut self, items: u64) -> Result<()> {
        let Some(left) = self.collection_items_left.checked_sub(items) else {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::CollectionItems,
                limit: self.limits.collection_items,
            });
        };
        self.collection_items_left = left;
        // A container walk is work: charge it as steps too, so the deadline
        // and cancellation are observed during a long comparison.
        self.step(items)
    }

    pub(crate) fn charge_function_call(&mut self, cost: u64) -> Result<()> {
        let Some(left) = self.function_calls_left.checked_sub(1) else {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::FunctionCalls,
                limit: u64::from(self.limits.function_calls),
            });
        };
        self.function_calls_left = left;
        self.step(cost.max(1))
    }

    #[cfg(feature = "regex")]
    pub(crate) fn charge_steps_for_match(&mut self, input_len: usize) -> Result<()> {
        self.step(1u64.saturating_add((input_len as u64) / 64))
    }

    #[cfg(feature = "regex")]
    pub(crate) fn charge_steps_for_regex_compile(&mut self, pattern_len: usize) -> Result<()> {
        self.step(32u64.saturating_add((pattern_len as u64) / 8))
    }

    pub(crate) fn check_time(&mut self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return Err(Error::DeadlineExceeded);
        }
        Ok(())
    }

    /// Validate one function result against its spec and charge its bytes as
    /// allocation. The final output budget is charged separately.
    pub(crate) fn check_value(&mut self, value: &Value, max_output: usize) -> Result<()> {
        let mut limits = self.limits.value;
        limits.total_bytes = limits.total_bytes.min(max_output);
        limits.string_bytes = limits.string_bytes.min(max_output);

        let mut meter = ValueMeter::rejecting_non_finite(&limits);
        meter.measure(value)?;
        let bytes = meter.bytes();
        // The output budget belongs to the final expression result; a
        // function result is bounded by its spec and charged as allocation.
        self.charge_allocation(bytes)
    }

    /// Validate the final result against the value limits, then charge it to
    /// the separate output budget. Keeping the two checks distinct gives a
    /// value error for an oversized structure and an output error for a value
    /// that is structurally valid but past the profile's output allowance.
    pub(crate) fn check_output_value(&mut self, value: &Value) -> Result<()> {
        let mut meter = ValueMeter::rejecting_non_finite(&self.limits.value);
        meter.measure(value)?;
        let bytes = meter.bytes();
        self.charge_output(bytes)
    }
}

/// Evaluate a compiled arena with an explicit frame stack. No AST shape can
/// turn into Rust call-stack depth here.
pub(crate) fn eval(
    nodes: &[Node],
    root: u32,
    context: &Context,
    budget: &mut EvalBudget,
    regex: &RegexPool,
) -> Result<Value> {
    Evaluator {
        nodes,
        context,
        budget,
        regex,
        call_counts: HashMap::new(),
    }
    .run(root)
}

struct Evaluator<'a> {
    nodes: &'a [Node],
    context: &'a Context,
    budget: &'a mut EvalBudget,
    regex: &'a RegexPool,
    call_counts: HashMap<Arc<str>, u32>,
}

/// One pending action. Values are kept on a separate stack; a frame says what
/// to do with them next.
enum Frame {
    Eval(u32),
    Not,
    Neg,
    Binary(BinOp),
    /// The left operand has been evaluated; the frame's node is the right one.
    ShortCircuit(BinOp, u32),
    /// The right operand of a non-short-circuited `&&`/`||` is on the stack.
    CheckBool(BinOp),
    Member(Arc<str>),
    Index,
    Call {
        name: Arc<str>,
        argc: usize,
    },
    Method {
        name: Arc<str>,
        argc: usize,
    },
}

impl<'a> Evaluator<'a> {
    fn run(&mut self, root: u32) -> Result<Value> {
        self.budget.check_time()?;

        let mut frames = Vec::with_capacity(16);
        let mut values = Vec::with_capacity(8);
        frames.push(Frame::Eval(root));

        while let Some(frame) = frames.pop() {
            self.budget.step(1)?;

            match frame {
                Frame::Eval(index) => self.schedule(index, &mut frames, &mut values)?,
                Frame::Not => {
                    let value = values.pop().expect("eval stack");
                    let boolean = boolean_of(&value, "!")?;
                    values.push(Value::Bool(!boolean));
                }
                Frame::Neg => {
                    let value = values.pop().expect("eval stack");
                    values.push(match value {
                        Value::Int(i) => i
                            .checked_neg()
                            .map(Value::Int)
                            .ok_or(Error::Overflow { operation: "-" })?,
                        Value::Float(f) => Value::Float(-f),
                        other => return Err(type_error("-", &other)),
                    });
                }
                Frame::Binary(op) => {
                    let right = values.pop().expect("eval stack");
                    let left = values.pop().expect("eval stack");
                    values.push(self.binary(op, &left, &right)?);
                }
                Frame::ShortCircuit(op, right) => {
                    let left = values.pop().expect("eval stack");
                    let boolean = boolean_of(&left, op.symbol())?;
                    let short = match op {
                        BinOp::And => !boolean,
                        BinOp::Or => boolean,
                        _ => unreachable!("only `&&` and `||` short-circuit"),
                    };

                    if short {
                        values.push(Value::Bool(short));
                    } else {
                        frames.push(Frame::CheckBool(op));
                        frames.push(Frame::Eval(right));
                    }
                }
                Frame::CheckBool(op) => {
                    let value = values.pop().expect("eval stack");
                    values.push(Value::Bool(boolean_of(&value, op.symbol())?));
                }
                Frame::Member(name) => {
                    let value = values.pop().expect("eval stack");
                    let map = value.as_map().ok_or_else(|| type_error(".", &value))?;
                    let member = map
                        .get(name.as_ref())
                        .cloned()
                        .ok_or_else(|| Error::Missing {
                            name: name.to_string(),
                        })?;
                    values.push(member);
                }
                Frame::Index => {
                    let key = values.pop().expect("eval stack");
                    let container = values.pop().expect("eval stack");
                    values.push(index(&container, &key)?);
                }
                Frame::Call { name, argc } => {
                    let args = take_args(&mut values, argc);
                    let function =
                        self.context
                            .get(&name)
                            .cloned()
                            .ok_or_else(|| Error::Unknown {
                                name: name.to_string(),
                            })?;
                    if !function.is_function() {
                        return Err(type_error("call", &function));
                    }
                    let spec = self.context.spec(&name);
                    let value = self.call(&name, &function, &args, spec)?;
                    values.push(value);
                }
                Frame::Method { name, argc } => {
                    let args = take_args(&mut values, argc);
                    let receiver = values.pop().expect("eval stack");
                    let value = self.method(&name, receiver, &args)?;
                    values.push(value);
                }
            }
        }

        Ok(values.pop().expect("eval stack has the root value"))
    }

    fn schedule(
        &mut self,
        index: u32,
        frames: &mut Vec<Frame>,
        values: &mut Vec<Value>,
    ) -> Result<()> {
        match &self.nodes[index as usize] {
            Node::Null => values.push(Value::Null),
            Node::Bool(b) => values.push(Value::Bool(*b)),
            Node::Int(i) => values.push(Value::Int(*i)),
            Node::Float(f) => values.push(Value::Float(*f)),
            Node::Str(s) => values.push(Value::Str(s.clone())),
            Node::Ident(name) => {
                values.push(
                    self.context
                        .get(name)
                        .cloned()
                        .ok_or_else(|| Error::Unknown {
                            name: name.to_string(),
                        })?,
                )
            }
            Node::Member(target, name) => {
                frames.push(Frame::Member(name.clone()));
                frames.push(Frame::Eval(*target));
            }
            Node::Index(target, key) => {
                frames.push(Frame::Index);
                frames.push(Frame::Eval(*key));
                frames.push(Frame::Eval(*target));
            }
            Node::Call(name, args) => {
                frames.push(Frame::Call {
                    name: name.clone(),
                    argc: args.len(),
                });
                for arg in args.iter().rev() {
                    frames.push(Frame::Eval(*arg));
                }
            }
            Node::Method(receiver, name, args) => {
                frames.push(Frame::Method {
                    name: name.clone(),
                    argc: args.len(),
                });
                for arg in args.iter().rev() {
                    frames.push(Frame::Eval(*arg));
                }
                frames.push(Frame::Eval(*receiver));
            }
            Node::Not(operand) => {
                frames.push(Frame::Not);
                frames.push(Frame::Eval(*operand));
            }
            Node::Neg(operand) => {
                frames.push(Frame::Neg);
                frames.push(Frame::Eval(*operand));
            }
            Node::Binary(op @ (BinOp::And | BinOp::Or), left, right) => {
                frames.push(Frame::ShortCircuit(*op, *right));
                frames.push(Frame::Eval(*left));
            }
            Node::Binary(op, left, right) => {
                frames.push(Frame::Binary(*op));
                frames.push(Frame::Eval(*right));
                frames.push(Frame::Eval(*left));
            }
        }

        Ok(())
    }

    fn binary(&mut self, op: BinOp, left: &Value, right: &Value) -> Result<Value> {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                self.arithmetic(op, left, right)
            }
            BinOp::Eq => Ok(Value::Bool(equals_budgeted(left, right, self.budget)?)),
            BinOp::Ne => Ok(Value::Bool(!equals_budgeted(left, right, self.budget)?)),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let ordering = left
                    .compare(right)
                    .ok_or_else(|| type_error(op.symbol(), blame(left, right)))?;

                Ok(Value::Bool(match op {
                    BinOp::Lt => ordering.is_lt(),
                    BinOp::Le => ordering.is_le(),
                    BinOp::Gt => ordering.is_gt(),
                    _ => ordering.is_ge(),
                }))
            }
            BinOp::And | BinOp::Or => unreachable!("short-circuited by the frame"),
        }
    }

    /// `+`, `-`, `*`, `/` and `%`. Two ints stay an int and keep CEL's
    /// semantics (`/` truncates, `%` is the remainder, a result past `i64`
    /// overflows); a `uint` on either side is computed in 128 bits, exact, and
    /// narrowed back to whichever integer kind holds the result; a float
    /// either side makes it a float. The operator that reads in the source is
    /// the one the error names.
    fn arithmetic(&mut self, op: BinOp, left: &Value, right: &Value) -> Result<Value> {
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

        // A `uint` on either side: computed in 128 bits, exact, and narrowed
        // back to the kind that holds the result.
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
                let len = a.len().checked_add(b.len()).ok_or(Error::ValueLimit {
                    what: "string value",
                    limit: self.budget.value_limits().string_bytes,
                })?;
                if len > self.budget.value_limits().string_bytes {
                    return Err(Error::ValueLimit {
                        what: "string value",
                        limit: self.budget.value_limits().string_bytes,
                    });
                }
                self.budget.charge_allocation(len)?;

                let mut text = String::with_capacity(len);
                text.push_str(a);
                text.push_str(b);

                return Ok(Value::Str(Arc::from(text)));
            }

            return Err(type_error(symbol, blame(left, right)));
        };

        if matches!(op, BinOp::Div | BinOp::Rem) && b == 0.0 {
            return Err(Error::DivideByZero { operation: symbol });
        }

        let result = match op {
            BinOp::Add => a + b,
            BinOp::Sub => a - b,
            BinOp::Mul => a * b,
            BinOp::Div => a / b,
            BinOp::Rem => a % b,
            _ => unreachable!("only arithmetic reaches here"),
        };
        if !result.is_finite() {
            return Err(Error::NonFinite { operation: symbol });
        }

        Ok(Value::Float(result))
    }

    fn call(
        &mut self,
        name: &Arc<str>,
        function: &Value,
        args: &[Value],
        spec: FunctionSpec,
    ) -> Result<Value> {
        let Value::Function(function) = function else {
            return Err(type_error("call", function));
        };

        self.budget.charge_function_call(spec.base_cost)?;
        if spec.purity == FunctionPurity::Effectful && !self.budget.allow_effectful_functions() {
            return Err(Error::FunctionRejected {
                name: name.to_string(),
                reason: "effectful functions are disabled by the evaluation profile",
            });
        }

        let count = self.call_counts.entry(Arc::clone(name)).or_insert(0);
        *count = count.saturating_add(1);
        if *count > spec.max_calls {
            return Err(Error::BudgetExceeded {
                kind: BudgetKind::FunctionCalls,
                limit: u64::from(spec.max_calls),
            });
        }

        self.budget
            .charge_allocation(args.len().saturating_mul(std::mem::size_of::<Value>()))?;

        let value = catch_unwind(AssertUnwindSafe(|| function(args))).map_err(|_| {
            Error::FunctionPanicked {
                name: name.to_string(),
            }
        })??;
        self.budget.check_time()?;
        self.budget.check_value(&value, spec.max_output_bytes)?;

        Ok(value)
    }

    fn method(&mut self, name: &Arc<str>, receiver: Value, args: &[Value]) -> Result<Value> {
        // The object's own method first — that is what makes an injected
        // object carry its methods — then a method of that name from the
        // context, which is the `size(x)` / `x.size()` pairing, and last the
        // built-in methods a value type carries of its own
        // (`name.length()`, `items.contains(x)`).
        let own = receiver
            .as_map()
            .and_then(|map| map.get(name.as_ref()))
            .filter(|value| value.is_function())
            .cloned();

        if let Some(function) = own {
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(receiver);
            call_args.extend_from_slice(args);
            return self.call(name, &function, &call_args, FunctionSpec::default());
        }

        if let Some(function) = self.context.get(name).cloned() {
            if !function.is_function() {
                return Err(Error::UnknownMethod {
                    name: name.to_string(),
                });
            }
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(receiver);
            call_args.extend_from_slice(args);
            let spec = self.context.spec(name);
            return self.call(name, &function, &call_args, spec);
        }

        let mut state = crate::methods::CallState::new(&mut *self.budget, self.regex);
        if let Some(value) = crate::methods::call(name, &receiver, args, &mut state)? {
            return Ok(value);
        }

        Err(Error::UnknownMethod {
            name: name.to_string(),
        })
    }
}

fn take_args(values: &mut Vec<Value>, argc: usize) -> Vec<Value> {
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        args.push(values.pop().expect("eval stack"));
    }
    args.reverse();
    args
}

fn index(container: &Value, key: &Value) -> Result<Value> {
    match (container, key) {
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
        _ => Err(type_error("[]", container)),
    }
}

/// Structural equality with the evaluator's budget: a list or map comparison
/// walks iteratively and charges every candidate pair.
pub(crate) fn equals_budgeted(
    left: &Value,
    right: &Value,
    budget: &mut EvalBudget,
) -> Result<bool> {
    let mut stack = vec![(left, right)];

    while let Some((left, right)) = stack.pop() {
        budget.charge_items(1)?;

        match (left, right) {
            (Value::Null, Value::Null) => {}
            (Value::Bool(a), Value::Bool(b)) if a == b => {}
            (Value::Str(a), Value::Str(b)) if a == b => {}
            (Value::Int(a), Value::Int(b)) if a == b => {}
            (Value::UInt(a), Value::UInt(b)) if a == b => {}
            (Value::Float(a), Value::Float(b)) if a == b => {}
            (Value::Int(a), Value::UInt(b)) | (Value::UInt(b), Value::Int(a))
                if i128::from(*a) == i128::from(*b) => {}
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a))
                if (*a as f64) == *b => {}
            (Value::UInt(a), Value::Float(b)) | (Value::Float(b), Value::UInt(a))
                if (*a as f64) == *b => {}
            (Value::List(a), Value::List(b)) => {
                if std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)) {
                    continue;
                }
                if a.len() != b.len() {
                    return Ok(false);
                }
                stack.extend(a.iter().zip(b.iter()));
            }
            (Value::Map(a), Value::Map(b)) => {
                if std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)) {
                    continue;
                }
                if a.len() != b.len() {
                    return Ok(false);
                }
                for (key, value) in a.iter() {
                    let Some(other) = b.get(key) else {
                        return Ok(false);
                    };
                    stack.push((value, other));
                }
            }
            (Value::Function(a), Value::Function(b)) if Arc::ptr_eq(a, b) => {}
            _ => return Ok(false),
        }
    }

    Ok(true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Expr;

    fn eval(source: &str, context: &Context) -> Result<Value> {
        Expr::compile(source).unwrap().eval(context)
    }

    fn error(source: &str, context: &Context) -> Error {
        eval(source, context).expect_err(source)
    }

    #[test]
    fn context_get_answers_both_ways() {
        let mut context = Context::new();
        assert_eq!(context.get("a"), None);
        context.set("a", 1);
        assert_eq!(context.get("a"), Some(&Value::Int(1)));
    }

    #[test]
    fn index_errors_name_the_missing_key_and_the_wrong_key_kind() {
        let mut context = Context::new();
        context
            .set("obj", Value::map([("k", 1)]))
            .set("items", Value::list([1]));

        assert_eq!(
            error("obj['nope']", &context),
            Error::Missing {
                name: "nope".to_string()
            }
        );
        assert_eq!(
            error("items[true]", &context),
            Error::Type {
                operation: "[]",
                got: "list"
            }
        );
        assert_eq!(
            error("obj[0]", &context),
            Error::Type {
                operation: "[]",
                got: "map"
            }
        );
        assert_eq!(
            error("null[0]", &context),
            Error::Type {
                operation: "[]",
                got: "null"
            }
        );
        // The index expression itself is evaluated and can fail.
        assert_eq!(
            error("items[missing]", &context),
            Error::Unknown {
                name: "missing".to_string()
            }
        );
    }

    #[test]
    fn member_access_requires_a_map() {
        let mut context = Context::new();
        context
            .set("obj", Value::map([("k", 1)]))
            .set("items", Value::list([1]));

        assert_eq!(
            error("items.k", &context),
            Error::Type {
                operation: ".",
                got: "list"
            }
        );
        assert_eq!(
            error("obj.missing", &context),
            Error::Missing {
                name: "missing".to_string()
            }
        );
    }

    #[test]
    fn calls_check_the_name_is_a_function() {
        let mut context = Context::new();
        context
            .set("add", Value::function(|args| Ok(Value::from(args.len()))))
            .set("value", 1);

        assert_eq!(eval("add()", &context).unwrap(), Value::from(0));
        assert_eq!(eval("add(1, 2)", &context).unwrap(), Value::from(2));
        assert_eq!(
            error("value()", &context),
            Error::Type {
                operation: "call",
                got: "int"
            }
        );
        assert_eq!(
            error("missing()", &context),
            Error::Unknown {
                name: "missing".to_string()
            }
        );
    }

    #[test]
    fn a_method_falls_through_to_unknown() {
        let mut context = Context::new();
        context.set("obj", Value::map([("k", 1)])).set("value", 2);

        // A member that is not a function does not answer the call.
        assert_eq!(
            error("obj.k()", &context),
            Error::UnknownMethod {
                name: "k".to_string()
            }
        );
        // A context name that is not a function does not answer it either.
        assert_eq!(
            error("obj.value()", &context),
            Error::UnknownMethod {
                name: "value".to_string()
            }
        );
    }

    #[test]
    fn unary_operators_evaluate_both_types() {
        assert_eq!(eval("-1", &Context::new()).unwrap(), Value::Int(-1));
        assert_eq!(eval("-1.5", &Context::new()).unwrap(), Value::Float(-1.5));
        assert_eq!(eval("!false", &Context::new()).unwrap(), Value::Bool(true));
        assert_eq!(
            error("-'s'", &Context::new()),
            Error::Type {
                operation: "-",
                got: "string"
            }
        );
        assert_eq!(
            error("!'s'", &Context::new()),
            Error::Type {
                operation: "!",
                got: "string"
            }
        );
    }

    #[test]
    fn logic_evaluates_the_right_side_and_checks_both() {
        let context = Context::new();
        assert_eq!(eval("true && true", &context).unwrap(), Value::Bool(true));
        assert_eq!(eval("true && false", &context).unwrap(), Value::Bool(false));
        assert_eq!(eval("false || true", &context).unwrap(), Value::Bool(true));
        assert_eq!(
            eval("false || false", &context).unwrap(),
            Value::Bool(false)
        );

        assert_eq!(
            error("true && 1", &context),
            Error::Type {
                operation: "&&",
                got: "int"
            }
        );
        assert_eq!(
            error("1 || true", &context),
            Error::Type {
                operation: "||",
                got: "int"
            }
        );
        assert_eq!(
            error("false || 1", &context),
            Error::Type {
                operation: "||",
                got: "int"
            }
        );
    }

    #[test]
    fn arithmetic_handles_every_operator_on_uints() {
        let mut context = Context::new();
        context.set("big", u64::MAX);

        assert_eq!(
            eval("big + 1", &context),
            Err(Error::Overflow { operation: "+" })
        );
        assert_eq!(
            eval("big / 0", &context),
            Err(Error::DivideByZero { operation: "/" })
        );
        assert_eq!(
            eval("big % 0", &context),
            Err(Error::DivideByZero { operation: "%" })
        );
        assert_eq!(
            eval("big / 2", &context).unwrap(),
            Value::UInt(u64::MAX / 2)
        );
        assert_eq!(eval("big - big", &context).unwrap(), Value::Int(0));
        assert_eq!(
            eval("big - 1.0", &context).unwrap(),
            Value::Float(u64::MAX as f64 - 1.0)
        );
        assert_eq!(
            error("big + true", &context),
            Error::Type {
                operation: "+",
                got: "bool"
            }
        );
    }

    #[test]
    fn integer_overflow_and_float_division_are_errors() {
        let context = Context::new();
        assert_eq!(
            eval("9223372036854775807 + 1", &context),
            Err(Error::Overflow { operation: "+" })
        );
        assert_eq!(
            eval("1.0 / 0.0", &context),
            Err(Error::DivideByZero { operation: "/" })
        );
        assert_eq!(
            eval("1.0 % 0.0", &context),
            Err(Error::DivideByZero { operation: "%" })
        );
    }

    #[test]
    fn comparison_blames_the_operand_without_an_overload() {
        let context = Context::new();
        assert_eq!(
            error("true < 1", &context),
            Error::Type {
                operation: "<",
                got: "bool"
            }
        );
        assert_eq!(
            error("'a' < 1", &context),
            Error::Type {
                operation: "<",
                got: "int"
            }
        );
        assert_eq!(eval("1 < 2.5", &context).unwrap(), Value::Bool(true));
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::{BudgetProfile, EvalLimits, Expr};

    fn task_budget() -> EvalBudget {
        EvalBudget::for_profile(BudgetProfile::Task)
    }

    #[test]
    fn context_surface_covers_mutation_iteration_and_validation() {
        let mut context = Context::new();
        assert!(context.is_empty());
        assert_eq!(context.len(), 0);
        context.set("a", 1);
        assert!(context.contains("a"));
        assert_eq!(context.len(), 1);
        assert_eq!(context.names().collect::<Vec<_>>(), ["a"]);
        context.set_spec("a", FunctionSpec::pure());
        assert_eq!(context.spec("a").purity, FunctionPurity::Pure);
        assert_eq!(context.remove("a"), Some(Value::Int(1)));
        assert_eq!(context.remove("a"), None);

        context.set("b", 1).set("c", 2);
        context.clear();
        assert!(context.is_empty());
        assert_eq!(context.spec("missing"), FunctionSpec::default());

        let context: Context = [("a", Value::from(1)), ("b", Value::from("x"))]
            .into_iter()
            .collect();
        assert_eq!(context.len(), 2);
        assert!(context.validate(&ValueLimits::default()).is_ok());

        let mut bad = Context::new();
        bad.set("nan", Value::Float(f64::NAN));
        assert_eq!(
            bad.validate(&ValueLimits::default()).unwrap_err(),
            Error::NonFinite { operation: "value" }
        );
    }

    #[test]
    fn budget_accessors_and_every_charge_error() {
        let mut budget = task_budget();
        assert!(budget.remaining_steps() > 0);
        assert!(budget.remaining_output_bytes() > 0);
        assert!(budget.remaining_function_calls() > 0);
        assert!(budget.value_limits().nodes > 0);
        assert!(!budget.allow_effectful_functions());
        budget.set_allow_effectful_functions(true);
        assert!(budget.allow_effectful_functions());

        let cancellation = budget.cancellation();
        assert!(!cancellation.is_cancelled());
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
        assert_eq!(budget.check_time(), Err(Error::Cancelled));

        budget.step(1).unwrap();
        budget.charge_allocation(1).unwrap();
        budget.charge_output(1).unwrap();
        budget.charge_items(1).unwrap();
        budget.charge_function_call(1).unwrap();

        let base = BudgetProfile::Task.eval_limits();
        let mut no_steps = EvalBudget::with_limits(EvalLimits { steps: 0, ..base });
        assert!(matches!(
            no_steps.step(1),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Steps,
                ..
            })
        ));
        let mut no_allocation = EvalBudget::with_limits(EvalLimits {
            allocation_bytes: 0,
            ..base
        });
        assert!(matches!(
            no_allocation.charge_allocation(1),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Allocation,
                ..
            })
        ));
        let mut no_output = EvalBudget::with_limits(EvalLimits {
            output_bytes: 0,
            ..base
        });
        assert!(matches!(
            no_output.charge_output(1),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Output,
                ..
            })
        ));
        let mut no_items = EvalBudget::with_limits(EvalLimits {
            collection_items: 0,
            ..base
        });
        assert!(matches!(
            no_items.charge_items(1),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::CollectionItems,
                ..
            })
        ));
        let mut no_calls = EvalBudget::with_limits(EvalLimits {
            function_calls: 0,
            ..base
        });
        assert!(matches!(
            no_calls.charge_function_call(1),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::FunctionCalls,
                ..
            })
        ));

        let mut deadline = EvalBudget::with_limits(EvalLimits {
            deadline: std::time::Duration::ZERO,
            ..base
        });
        assert_eq!(deadline.check_time(), Err(Error::DeadlineExceeded));

        let mut value_budget = task_budget();
        value_budget.check_value(&Value::from("x"), 8).unwrap();
        assert!(matches!(
            value_budget.check_value(&Value::from("xxxxxxxx"), 2),
            Err(Error::ValueLimit { .. })
        ));
        let mut output_budget = EvalBudget::with_limits(EvalLimits {
            output_bytes: 0,
            ..base
        });
        assert!(matches!(
            output_budget.check_output_value(&Value::from("x")),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Output,
                ..
            })
        ));
        let mut finite = task_budget();
        assert_eq!(
            finite.check_value(&Value::Float(f64::NAN), 8).unwrap_err(),
            Error::NonFinite { operation: "value" }
        );
    }

    #[test]
    fn budgeted_equality_covers_every_value_pair() {
        let fun = Value::function(|_| Ok(Value::Null));
        let fun2 = Value::function(|_| Ok(Value::Null));
        let mut context = Context::new();
        context
            .set("n", Value::Null)
            .set("t", true)
            .set("s", "x")
            .set("i", 1)
            .set("u1", Value::UInt(1))
            .set("u2", Value::UInt(2))
            .set("f1", 1.0)
            .set("f2", 2.0)
            .set("l", Value::list([1, 2]))
            .set("l2", Value::list([1, 2]))
            .set("l3", Value::list([1, 2, 3]))
            .set("m", Value::map([("a", 1), ("b", 2)]))
            .set("m2", Value::map([("a", 1), ("b", 2)]))
            .set("m3", Value::map([("a", 1), ("c", 2)]))
            .set("m4", Value::map([("a", 1)]))
            .set("fun", fun)
            .set("fun2", fun2);

        for source in [
            "n == n",
            "t == t",
            "s == s",
            "i == i",
            "u1 == u1",
            "f1 == f1",
            "i == u1",
            "u1 == i",
            "i == f1",
            "f1 == i",
            "u1 == f1",
            "f1 == u1",
            "l == l",
            "l == l2",
            "l != l3",
            "m == m",
            "m == m2",
            "m != m3",
            "m != m4",
            "fun == fun",
            "fun != fun2",
            "n != t",
            "u1 != u2",
            "f1 != f2",
        ] {
            let value = Expr::compile(source)
                .unwrap()
                .eval(&context)
                .unwrap_or_else(|error| panic!("{source}: {error}"));
            assert!(value.as_bool().is_some(), "{source}");
        }
    }

    #[test]
    fn function_argument_allocation_is_charged() {
        let mut context = Context::new();
        context.set("f", Value::function(|_| Ok(Value::Null)));
        let expr = Expr::compile("f(1, 2, 3)").unwrap();
        let mut budget = EvalBudget::with_limits(EvalLimits {
            allocation_bytes: 0,
            ..BudgetProfile::Task.eval_limits()
        });

        assert!(matches!(
            expr.eval_with_budget(&context, &mut budget),
            Err(Error::BudgetExceeded {
                kind: BudgetKind::Allocation,
                ..
            })
        ));
    }
}
