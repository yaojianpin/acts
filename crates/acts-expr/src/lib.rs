//! A small, resource-bounded expression evaluator for the expressions a
//! workflow carries.
//!
//! `acts-expr` compiles a source string once into an [`Expr`] and evaluates it
//! against a [`Context`] as often as needed — the same compiled expression can
//! be evaluated from any number of threads with different contexts. It has no
//! dependencies, allocates one `Vec` per compiled expression (nodes reference
//! their children by index, not by pointer) and never touches a global.
//!
//! # The expression surface
//!
//! - literals: `12`, `1.5`, `1e3`, `'text'`, `"text"`, `true`, `false`, `null`
//! - arithmetic: `+ - * / %`, unary `-`, with `( )` to group
//! - comparison: `== != < <= > >=` (which do not chain, so `1 < 2 < 3` is a
//!   parse error rather than a type error)
//! - logic: `&&`, `||`, unary `!`, both short-circuiting
//! - access: `a.b`, `a['b']`, `list[0]`, and calls `f(x)`, `a.f(x)`
//!
//! `==` compares numbers across `int`, `uint` and `float`, lists and objects
//! element-wise, and values of different kinds are not equal (so a workflow
//! that compares a missing value with a string is `false`, not an error).
//! Arithmetic requires numbers, `+` also concatenates two strings, and
//! `&& || !` require bools. Two ints stay an int (so `/` truncates and `%` is
//! the remainder) and an overflow is an error; a `uint` — what JSON gives a
//! host for a value above `i64::MAX` — stays exact against an int, and any
//! float either side makes the result a float. A non-finite float is an
//! error, at a literal, a computed result or an injected value.
//!
//! # Bounded compilation and evaluation
//!
//! An expression is user data that runs on the scheduler's hot path, so every
//! stage has a budget:
//!
//! - [`CompileLimits`] bounds source bytes, tokens, nodes, logical AST depth,
//!   literal and identifier bytes, call arguments and regex patterns. The AST
//!   depth includes left-deep binary chains (`1 + 1 + ...`), not just grouped
//!   operands. String literals joined by `+` are folded under the literal
//!   budget; an invalid constant `is_match` pattern fails compilation.
//! - [`EvalBudget`] bounds steps, cumulative allocation, output, collection
//!   walks, function calls, wall time and cancellation. And/Or keep their
//!   short circuit; every other node consumes at least one step. A budget
//!   error is an error, never `false`.
//! - [`ValueLimits`] bounds the values a context injects: depth, nodes, list
//!   items, map entries, key and string bytes, total bytes. They are checked
//!   before evaluation and after every function result.
//! - The evaluator uses explicit frame and value stacks: AST depth never
//!   becomes Rust call-stack depth.
//!
//! [`BudgetProfile`] bundles the recommended compile and evaluation limits for
//! a gateway condition, task parameter, query filter and assignment. The
//! [`ExprCache`] keys a compiled expression by source, semantics version,
//! function registry generation, expected type, profile and compile limits.
//!
//! # Built-in methods
//!
//! A value carries a few methods of its own, so the common test needs nothing
//! injected:
//!
//! - a string: `length()`, `startsWith(p)`, `endsWith(p)`, `indexOf(p)`,
//!   `contains(p)`, and `is_match(pattern)` — a regular expression, behind the
//!   `regex` feature
//! - a list: `length()`, `contains(value)` (an element, compared by value)
//! - a map: `length()` (its entries)
//!
//! `length()` and `indexOf()` count characters, not bytes. These are found
//! last: the object's own method, then a function of that name in the context,
//! then the built-in — so a host that injects `length` defines what it means.
//!
//! # Injection
//!
//! Everything an expression reads is in the [`Context`]: scalars, lists and
//! objects ([`Value::Map`]) as values, and methods as [`Value::Function`]s.
//! A function is called as `f(x)`, or as a method with its receiver first —
//! `x.f()` — and a function stored *inside* an object is called that way
//! through the object, so an injected object carries its own methods.
//! [`Context::set_spec`] attaches a [`FunctionSpec`] to a name; a function
//! without one gets the default (pure context read, one step, 1000 calls and a
//! 1 MiB output). A function call is isolated from panics, but a function that
//! blocks, performs I/O or allocates beyond its declared output is a host
//! contract violation the evaluator can detect only after the call.
//!
//! ```rust
//! use acts_expr::{Context, Expr, Value};
//!
//! let mut context = Context::new();
//! context
//!     .set("count", 3)
//!     .set("status", "ok")
//!     .set("step1", Value::map([
//!         ("value", Value::from(7)),
//!         ("id", Value::from("step1")),
//!     ]))
//!     .set(
//!         "upper",
//!         Value::function(|args| match args {
//!             [Value::Str(text)] => Ok(Value::from(text.to_uppercase())),
//!             _ => Err(acts_expr::Error::Type { operation: "upper", got: "other" }),
//!         }),
//!     );
//!
//! let expr = Expr::compile("step1.value + count * 2 >= 13 && status != 'bad'")?;
//! assert_eq!(expr.eval(&context)?, Value::Bool(true));
//!
//! // A method: `upper` from the context, called with `status` as its receiver.
//! let name = Expr::compile("status.upper()")?;
//! assert_eq!(name.eval(&context)?, Value::from("OK"));
//! # Ok::<_, acts_expr::Error>(())
//! ```
//!
//! # Deliberately not in the surface
//!
//! The evaluator answers the expressions a workflow writes — conditions,
//! arithmetic on task data, string and object access — and nothing more. There
//! is no assignment, no statement, no ternary (`?:`), no collection literal
//! (`[...]`, `{...}`), no `in`, no comprehension, and no method beyond the
//! built-ins above — every other method is one the host injected. Each of the
//! absent forms is rejected with a message that says so.

mod cache;
mod error;
mod eval;
mod lexer;
mod methods;
mod parser;
mod regex_pool;
mod value;

pub use cache::{CacheKey, CacheStats, ExprCache};
pub use error::{BudgetKind, Error, Result};
pub use eval::{Cancellation, Context, EvalBudget};
pub use value::{Map, NativeFunction, Value};

use parser::Node;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// The version of the parser/evaluator semantics a compiled [`Expr`] carries.
///
/// A cached artifact or a persisted expression must include this value: a
/// change to associativity, typing or method behavior is a new version, not a
/// silent reinterpretation of a running process.
pub const SEMANTICS_VERSION: u32 = 1;

/// Hard limits for reading an expression source.
///
/// Every field is checked before or while the AST is built. The defaults are
/// the task-parameter profile: generous for a workflow's `${{ }}` placeholder,
/// small enough that one expression cannot dominate a scheduler worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompileLimits {
    /// Bytes in the whole source.
    pub source_bytes: usize,
    /// Tokens produced by the lexer.
    pub tokens: usize,
    /// Nodes in the arena.
    pub nodes: usize,
    /// Logical AST height, including binary chains.
    pub depth: usize,
    /// Decoded bytes of one string literal.
    pub literal_bytes: usize,
    /// Bytes of one identifier or member name.
    pub identifier_bytes: usize,
    /// Arguments in one call.
    pub call_args: usize,
    /// Bytes of a constant regular-expression pattern.
    pub regex_pattern_bytes: usize,
}

impl Default for CompileLimits {
    fn default() -> Self {
        CompileLimits {
            source_bytes: 64 * 1024,
            tokens: 8192,
            nodes: 8192,
            depth: 128,
            literal_bytes: 16 * 1024,
            identifier_bytes: 256,
            call_args: 256,
            regex_pattern_bytes: 8 * 1024,
        }
    }
}

/// Limits for a value that crosses into, or out of, an expression.
///
/// A source budget does not bound the context: process variables, snapshots,
/// message parameters and function results can each carry a large or deeply
/// nested list or map. These limits are checked on the context before
/// evaluation, on every function result, and on the final result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueLimits {
    pub depth: usize,
    pub nodes: usize,
    pub list_items: usize,
    pub map_entries: usize,
    pub key_bytes: usize,
    pub string_bytes: usize,
    pub total_bytes: usize,
}

impl Default for ValueLimits {
    fn default() -> Self {
        ValueLimits {
            depth: 64,
            nodes: 100_000,
            list_items: 100_000,
            map_entries: 100_000,
            key_bytes: 1024,
            string_bytes: 1024 * 1024,
            total_bytes: 64 * 1024 * 1024,
        }
    }
}

/// The runtime budget one evaluation may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvalLimits {
    /// AST nodes and other evaluator steps.
    pub steps: u64,
    /// Cumulative bytes allocated by string concatenation and function calls.
    pub allocation_bytes: usize,
    /// Bytes in the final value. A function result has its own
    /// `FunctionSpec::max_output_bytes` and is charged as allocation.
    pub output_bytes: usize,
    /// Native function calls.
    pub function_calls: u32,
    /// Elements compared by `==` and `contains`.
    pub collection_items: u64,
    /// Wall-clock budget from the start of one evaluation.
    pub deadline: Duration,
    /// The value limits the context and results are checked against.
    pub value: ValueLimits,
    /// Whether a [`FunctionPurity::Effectful`] function may run. Off by
    /// default: a condition should not perform external side effects.
    pub allow_effectful_functions: bool,
}

impl Default for EvalLimits {
    fn default() -> Self {
        BudgetProfile::Task.eval_limits()
    }
}

/// The recommended limits for one expression use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BudgetProfile {
    /// A gateway/branch condition: small, bool-shaped, short deadline.
    Gateway,
    /// A task parameter or output template: the general-purpose profile.
    Task,
    /// One query filter. Reuse one [`EvalBudget`] across rows for a total
    /// per-query budget.
    Query,
    /// An owner or assignee expression: tiny output, short deadline.
    Assignment,
}

impl BudgetProfile {
    pub const ALL: [BudgetProfile; 4] = [
        BudgetProfile::Gateway,
        BudgetProfile::Task,
        BudgetProfile::Query,
        BudgetProfile::Assignment,
    ];

    /// The stable name used by cache keys, metrics and diagnostics.
    pub fn id(self) -> &'static str {
        match self {
            BudgetProfile::Gateway => "gateway",
            BudgetProfile::Task => "task",
            BudgetProfile::Query => "query",
            BudgetProfile::Assignment => "assignment",
        }
    }

    pub fn compile_limits(self) -> CompileLimits {
        match self {
            BudgetProfile::Gateway => CompileLimits {
                source_bytes: 16 * 1024,
                tokens: 4096,
                nodes: 2048,
                depth: 128,
                literal_bytes: 8 * 1024,
                identifier_bytes: 256,
                call_args: 64,
                regex_pattern_bytes: 4 * 1024,
            },
            BudgetProfile::Task => CompileLimits::default(),
            BudgetProfile::Query => CompileLimits {
                source_bytes: 8 * 1024,
                tokens: 2048,
                nodes: 1024,
                depth: 128,
                literal_bytes: 4 * 1024,
                identifier_bytes: 256,
                call_args: 32,
                regex_pattern_bytes: 2 * 1024,
            },
            BudgetProfile::Assignment => CompileLimits {
                source_bytes: 16 * 1024,
                tokens: 4096,
                nodes: 2048,
                depth: 128,
                literal_bytes: 8 * 1024,
                identifier_bytes: 256,
                call_args: 64,
                regex_pattern_bytes: 2 * 1024,
            },
        }
    }

    pub fn eval_limits(self) -> EvalLimits {
        let task_values = ValueLimits::default();
        match self {
            BudgetProfile::Gateway => EvalLimits {
                steps: 20_000,
                allocation_bytes: 1024 * 1024,
                output_bytes: 1024,
                function_calls: 64,
                collection_items: 10_000,
                deadline: Duration::from_millis(10),
                value: ValueLimits {
                    nodes: 10_000,
                    list_items: 10_000,
                    map_entries: 10_000,
                    string_bytes: 256 * 1024,
                    total_bytes: 1024 * 1024,
                    ..task_values
                },
                allow_effectful_functions: false,
            },
            BudgetProfile::Task => EvalLimits {
                steps: 100_000,
                allocation_bytes: 8 * 1024 * 1024,
                output_bytes: 1024 * 1024,
                function_calls: 1000,
                collection_items: 100_000,
                deadline: Duration::from_millis(50),
                value: task_values,
                allow_effectful_functions: false,
            },
            BudgetProfile::Query => EvalLimits {
                steps: 10_000,
                allocation_bytes: 512 * 1024,
                output_bytes: 1024,
                function_calls: 32,
                collection_items: 10_000,
                deadline: Duration::from_millis(10),
                value: ValueLimits {
                    nodes: 10_000,
                    list_items: 10_000,
                    map_entries: 10_000,
                    string_bytes: 256 * 1024,
                    total_bytes: 1024 * 1024,
                    ..task_values
                },
                allow_effectful_functions: false,
            },
            BudgetProfile::Assignment => EvalLimits {
                steps: 20_000,
                allocation_bytes: 1024 * 1024,
                output_bytes: 256,
                function_calls: 64,
                collection_items: 10_000,
                deadline: Duration::from_millis(10),
                value: ValueLimits {
                    nodes: 10_000,
                    list_items: 10_000,
                    map_entries: 10_000,
                    string_bytes: 64 * 1024,
                    total_bytes: 1024 * 1024,
                    ..task_values
                },
                allow_effectful_functions: false,
            },
        }
    }
}

/// The type a caller requires from an expression.
///
/// Compilation rejects a result whose type is statically known and wrong (a
/// literal, or an operator with a fixed result type); evaluation checks the
/// dynamic result before returning it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprType {
    Any,
    Bool,
    Int,
    Number,
    String {
        max_bytes: usize,
    },
    /// An integer number of milliseconds in `0..=max_ms`.
    Duration {
        max_ms: u64,
    },
}

impl ExprType {
    pub fn name(self) -> &'static str {
        match self {
            ExprType::Any => "any",
            ExprType::Bool => "bool",
            ExprType::Int => "int",
            ExprType::Number => "number",
            ExprType::String { .. } => "string",
            ExprType::Duration { .. } => "duration",
        }
    }

    /// Check a statically known result type at compile time. `Unknown` is
    /// left for the runtime check.
    fn check_static(self, nodes: &[Node], root: u32) -> Result<()> {
        if self == ExprType::Any {
            return Ok(());
        }

        let kind = static_kind(nodes, root);
        match (self, kind) {
            (_, StaticKind::Unknown) => Ok(()),
            (ExprType::Bool, StaticKind::Bool)
            | (ExprType::Int, StaticKind::Int)
            | (ExprType::Number, StaticKind::Int | StaticKind::Float)
            | (ExprType::Duration { .. }, StaticKind::Int) => Ok(()),
            (ExprType::String { max_bytes }, StaticKind::Str) => {
                if let Node::Str(text) = &nodes[root as usize]
                    && text.len() > max_bytes
                {
                    return Err(Error::TooLarge {
                        what: "string result",
                        limit: max_bytes,
                    });
                }
                Ok(())
            }
            _ => Err(Error::ResultType {
                expected: self.name(),
                got: kind.name(),
            }),
        }
    }

    /// Check the actual result and enforce the value's own boundary.
    fn check_value(self, value: &Value) -> Result<()> {
        match self {
            ExprType::Any => Ok(()),
            ExprType::Bool => value.as_bool().map(|_| ()).ok_or(Error::ResultType {
                expected: "bool",
                got: value.type_name(),
            }),
            ExprType::Int => value.as_int().map(|_| ()).ok_or(Error::ResultType {
                expected: "int",
                got: value.type_name(),
            }),
            ExprType::Number => {
                if !value.is_number() {
                    return Err(Error::ResultType {
                        expected: "number",
                        got: value.type_name(),
                    });
                }
                if let Value::Float(f) = value
                    && !f.is_finite()
                {
                    return Err(Error::NonFinite {
                        operation: "result",
                    });
                }
                Ok(())
            }
            ExprType::String { max_bytes } => match value {
                Value::Str(text) if text.len() <= max_bytes => Ok(()),
                Value::Str(_) => Err(Error::TooLarge {
                    what: "string result",
                    limit: max_bytes,
                }),
                other => Err(Error::ResultType {
                    expected: "string",
                    got: other.type_name(),
                }),
            },
            ExprType::Duration { max_ms } => {
                let Some(ms) = value.as_u64() else {
                    return Err(Error::ResultType {
                        expected: "duration",
                        got: value.type_name(),
                    });
                };
                if ms > max_ms {
                    return Err(Error::ValueLimit {
                        what: "duration result (ms)",
                        limit: usize::try_from(max_ms).unwrap_or(usize::MAX),
                    });
                }
                Ok(())
            }
        }
    }
}

/// Whether a native function is a pure computation, a context read or an
/// external effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionPurity {
    Pure,
    ContextRead,
    Effectful,
}

/// The contract an injected function declares.
///
/// The evaluator charges `base_cost` steps and one function call, enforces the
/// per-name call count and the output byte cap, and refuses an `Effectful`
/// function unless the profile opts in. It cannot preempt a blocking function;
/// a host that injects one must keep the synchronous contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionSpec {
    pub purity: FunctionPurity,
    pub base_cost: u64,
    pub max_calls: u32,
    pub max_output_bytes: usize,
    pub deterministic: bool,
    pub thread_safe: bool,
}

impl Default for FunctionSpec {
    fn default() -> Self {
        FunctionSpec {
            purity: FunctionPurity::ContextRead,
            base_cost: 1,
            max_calls: 1000,
            max_output_bytes: 1024 * 1024,
            deterministic: false,
            thread_safe: true,
        }
    }
}

impl FunctionSpec {
    /// A deterministic, side-effect-free function.
    pub fn pure() -> FunctionSpec {
        FunctionSpec {
            purity: FunctionPurity::Pure,
            deterministic: true,
            ..FunctionSpec::default()
        }
    }
}

/// A compiled expression.
///
/// Compiling is the only step that reads the source; evaluation is a walk over
/// the compiled nodes, so a host that evaluates the same expression per task —
/// a step condition, a parameter filled per process — compiles it once and
/// keeps the [`Expr`]. A compiled expression is immutable and can be shared
/// across threads.
pub struct Expr {
    nodes: Arc<[Node]>,
    root: u32,
    depth: u32,
    estimated_cost: u64,
    source_hash: u64,
    expected_type: ExprType,
    profile: BudgetProfile,
    regex: Arc<regex_pool::RegexPool>,
}

impl Expr {
    /// Compile `source` with the task profile and no required result type.
    pub fn compile(source: &str) -> Result<Expr> {
        Expr::build(
            source,
            ExprType::Any,
            BudgetProfile::Task,
            &CompileLimits::default(),
        )
    }

    /// Compile with explicit compile limits and the task evaluation profile.
    pub fn compile_with(source: &str, limits: &CompileLimits) -> Result<Expr> {
        Expr::build(source, ExprType::Any, BudgetProfile::Task, limits)
    }

    /// Compile with a required result type.
    pub fn compile_typed(source: &str, expected: ExprType) -> Result<Expr> {
        Expr::build(
            source,
            expected,
            BudgetProfile::Task,
            &CompileLimits::default(),
        )
    }

    /// Compile with explicit limits and a required result type.
    pub fn compile_typed_with(
        source: &str,
        expected: ExprType,
        limits: &CompileLimits,
    ) -> Result<Expr> {
        Expr::build(source, expected, BudgetProfile::Task, limits)
    }

    /// Compile with the limits and evaluation budget of a profile.
    pub fn compile_with_profile(source: &str, profile: BudgetProfile) -> Result<Expr> {
        Expr::build(source, ExprType::Any, profile, &profile.compile_limits())
    }

    /// Compile with a profile and a required result type.
    pub fn compile_typed_with_profile(
        source: &str,
        expected: ExprType,
        profile: BudgetProfile,
    ) -> Result<Expr> {
        Expr::build(source, expected, profile, &profile.compile_limits())
    }

    pub(crate) fn compile_typed_full(
        source: &str,
        expected: ExprType,
        profile: BudgetProfile,
        limits: &CompileLimits,
    ) -> Result<Expr> {
        Expr::build(source, expected, profile, limits)
    }

    fn build(
        source: &str,
        expected: ExprType,
        profile: BudgetProfile,
        limits: &CompileLimits,
    ) -> Result<Expr> {
        if source.len() > limits.source_bytes {
            return Err(Error::TooLarge {
                what: "source",
                limit: limits.source_bytes,
            });
        }

        let parsed = parser::Parser::new(source, *limits)?.parse()?;
        expected.check_static(&parsed.nodes, parsed.root)?;

        let regex = Arc::new(regex_pool::RegexPool::compile_constants(
            &parsed.nodes,
            limits,
        )?);
        let estimated_cost = parsed
            .nodes
            .iter()
            .fold(0u64, |total, node| total.saturating_add(node_cost(node)));

        Ok(Expr {
            nodes: parsed.nodes.into(),
            root: parsed.root,
            depth: parsed.depth,
            estimated_cost,
            source_hash: hash_source(source),
            expected_type: expected,
            profile,
            regex,
        })
    }

    /// Evaluate against `context` with the expression's profile budget.
    pub fn eval(&self, context: &Context) -> Result<Value> {
        let mut budget = EvalBudget::for_profile(self.profile);

        self.eval_with_budget(context, &mut budget)
    }

    /// Evaluate against `context` with a caller-owned budget.
    ///
    /// The budget is consumed, not replaced: a caller that evaluates a filter
    /// over many rows can reuse it for a total query budget. The expression's
    /// expected type still applies.
    pub fn eval_with_budget(&self, context: &Context, budget: &mut EvalBudget) -> Result<Value> {
        budget.check_time()?;
        context.validate(budget.value_limits())?;

        let value = eval::eval(&self.nodes, self.root, context, budget, self.regex.as_ref())?;
        budget.check_time()?;
        self.expected_type.check_value(&value)?;
        budget.check_output_value(&value)?;

        Ok(value)
    }

    /// The names this expression reads, deduplicated and sorted: variables,
    /// called functions, and the names of called methods (which a receiver may
    /// answer itself before the context is consulted).
    ///
    /// A host that injects on demand — rather than filling a context with
    /// everything an expression *might* read — injects exactly these.
    pub fn references(&self) -> Vec<&str> {
        let mut names = Vec::new();

        for node in self.nodes.iter() {
            match node {
                Node::Ident(name) => names.push(name.as_ref()),
                Node::Call(name, _) | Node::Method(_, name, _) => names.push(name.as_ref()),
                _ => {}
            }
        }

        names.sort_unstable();
        names.dedup();
        names
    }

    /// How many nodes the compiled expression holds, for a host that bounds
    /// what it accepts. Constant-folded operands remain in the arena; the
    /// count is a conservative compile-budget number, not a runtime cost.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The logical AST height the compiler accepted.
    pub fn depth(&self) -> usize {
        self.depth as usize
    }

    /// A conservative step estimate used by a deploy-time profile check.
    pub fn estimated_cost(&self) -> u64 {
        self.estimated_cost
    }

    /// The source hash a cache or a persisted failure record uses.
    pub fn source_hash(&self) -> u64 {
        self.source_hash
    }

    pub fn semantics_version(&self) -> u32 {
        SEMANTICS_VERSION
    }

    pub fn expected_type(&self) -> ExprType {
        self.expected_type
    }

    pub fn profile(&self) -> BudgetProfile {
        self.profile
    }
}

impl Clone for Expr {
    fn clone(&self) -> Expr {
        Expr {
            nodes: Arc::clone(&self.nodes),
            root: self.root,
            depth: self.depth,
            estimated_cost: self.estimated_cost,
            source_hash: self.source_hash,
            expected_type: self.expected_type,
            profile: self.profile,
            regex: Arc::clone(&self.regex),
        }
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Expr")
            .field("nodes", &self.nodes.len())
            .field("root", &self.root)
            .field("depth", &self.depth)
            .field("expected_type", &self.expected_type)
            .field("profile", &self.profile)
            .finish()
    }
}

/// Compile and evaluate in one step, for a caller that has no reason to keep
/// the compiled expression.
pub fn eval(source: &str, context: &Context) -> Result<Value> {
    Expr::compile(source)?.eval(context)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StaticKind {
    Null,
    Bool,
    Int,
    Float,
    Str,
    Other,
    Unknown,
}

impl StaticKind {
    fn name(self) -> &'static str {
        match self {
            StaticKind::Null => "null",
            StaticKind::Bool => "bool",
            StaticKind::Int => "int",
            StaticKind::Float => "float",
            StaticKind::Str => "string",
            StaticKind::Other => "other",
            StaticKind::Unknown => "unknown",
        }
    }
}

/// The type an operator's result is statically known to have. The AST depth is
/// bounded, so this walk is too.
fn static_kind(nodes: &[Node], index: u32) -> StaticKind {
    match &nodes[index as usize] {
        Node::Null => StaticKind::Null,
        Node::Bool(_) => StaticKind::Bool,
        Node::Int(_) => StaticKind::Int,
        Node::Float(_) => StaticKind::Float,
        Node::Str(_) => StaticKind::Str,
        Node::Ident(_) | Node::Member(..) | Node::Index(..) | Node::Call(..) | Node::Method(..) => {
            StaticKind::Unknown
        }
        Node::Not(_) => StaticKind::Bool,
        Node::Neg(operand) => match static_kind(nodes, *operand) {
            StaticKind::Int => StaticKind::Int,
            StaticKind::Float => StaticKind::Float,
            _ => StaticKind::Unknown,
        },
        Node::Binary(op, left, right) => match op {
            parser::BinOp::And
            | parser::BinOp::Or
            | parser::BinOp::Eq
            | parser::BinOp::Ne
            | parser::BinOp::Lt
            | parser::BinOp::Le
            | parser::BinOp::Gt
            | parser::BinOp::Ge => StaticKind::Bool,
            parser::BinOp::Add => match (static_kind(nodes, *left), static_kind(nodes, *right)) {
                (StaticKind::Str, StaticKind::Str) => StaticKind::Str,
                (StaticKind::Int, StaticKind::Int) => StaticKind::Int,
                _ => StaticKind::Other,
            },
            parser::BinOp::Sub | parser::BinOp::Mul | parser::BinOp::Div | parser::BinOp::Rem => {
                match (static_kind(nodes, *left), static_kind(nodes, *right)) {
                    (StaticKind::Int, StaticKind::Int) => StaticKind::Int,
                    _ => StaticKind::Other,
                }
            }
        },
    }
}

fn node_cost(node: &Node) -> u64 {
    match node {
        Node::Call(_, args) | Node::Method(_, _, args) => 1 + args.len() as u64,
        Node::Str(text) => 1 + (text.len() / 64) as u64,
        _ => 1,
    }
}

pub(crate) fn hash_source(source: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn a_compiled_expression_reports_its_size() {
        let expr = Expr::compile("1 + 2").unwrap();
        assert_eq!(expr.len(), 3);
        assert!(!expr.is_empty());

        let clone = expr.clone();
        assert_eq!(clone.eval(&Context::new()).unwrap(), Value::Int(3));
        assert!(format!("{expr:?}").starts_with("Expr {"));
    }

    #[test]
    fn the_one_step_eval_compiles_first() {
        let mut context = Context::new();
        context.set("a", 1);
        assert_eq!(eval("a + 1", &context).unwrap(), Value::Int(2));
        assert_eq!(eval("a +", &context).unwrap_err().position(), Some(3));
    }

    #[test]
    fn metadata_is_stable_and_source_sensitive() {
        let first = Expr::compile("1 + 2").unwrap();
        let second = Expr::compile("1 + 2").unwrap();
        let other = Expr::compile("1 + 3").unwrap();

        assert_eq!(first.source_hash(), second.source_hash());
        assert_ne!(first.source_hash(), other.source_hash());
        assert_eq!(first.semantics_version(), SEMANTICS_VERSION);
        assert_eq!(first.expected_type(), ExprType::Any);
        assert_eq!(first.profile(), BudgetProfile::Task);
        assert!(first.estimated_cost() >= 3);
        assert_eq!(first.depth(), 2);
    }

    #[test]
    fn typed_compile_rejects_a_statically_wrong_result() {
        assert!(Expr::compile_typed("1", ExprType::Bool).is_err());
        assert!(Expr::compile_typed("true", ExprType::Bool).is_ok());
        assert!(Expr::compile_typed("1 < 2", ExprType::Bool).is_ok());
        assert!(Expr::compile_typed("'abcdef'", ExprType::String { max_bytes: 3 }).is_err());
        assert!(Expr::compile_typed("'abc'", ExprType::String { max_bytes: 3 }).is_ok());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;

    fn assert_err(result: Result<Expr>) -> Error {
        result.expect_err("expected a compile error")
    }

    #[test]
    fn every_profile_publishes_its_limits_and_name() {
        assert_eq!(
            BudgetProfile::ALL.map(BudgetProfile::id),
            ["gateway", "task", "query", "assignment"]
        );

        for profile in BudgetProfile::ALL {
            let compile = profile.compile_limits();
            let eval = profile.eval_limits();

            assert!(compile.source_bytes > 0);
            assert!(compile.tokens > 0);
            assert!(compile.nodes > 0);
            assert!(compile.depth > 0);
            assert!(compile.literal_bytes > 0);
            assert!(compile.identifier_bytes > 0);
            assert!(compile.call_args > 0);
            assert!(compile.regex_pattern_bytes > 0);
            assert!(eval.steps > 0);
            assert!(eval.allocation_bytes > 0);
            assert!(eval.output_bytes > 0);
            assert!(eval.function_calls > 0);
            assert!(eval.collection_items > 0);
            assert!(!eval.allow_effectful_functions);
            assert!(eval.value.nodes > 0);
        }

        assert_eq!(EvalLimits::default().steps, 100_000);
        assert_eq!(
            CompileLimits::default(),
            BudgetProfile::Task.compile_limits()
        );
    }

    #[test]
    fn expression_type_names_and_static_checks() {
        assert_eq!(ExprType::Any.name(), "any");
        assert_eq!(ExprType::Bool.name(), "bool");
        assert_eq!(ExprType::Int.name(), "int");
        assert_eq!(ExprType::Number.name(), "number");
        assert_eq!(ExprType::String { max_bytes: 1 }.name(), "string");
        assert_eq!(ExprType::Duration { max_ms: 1 }.name(), "duration");

        let strings = "a".repeat(9000);
        let source = format!("'{strings}' + '{strings}'");
        let accepted = [
            ("null", ExprType::Any),
            ("true", ExprType::Bool),
            ("1", ExprType::Int),
            ("1.5", ExprType::Number),
            ("'a'", ExprType::String { max_bytes: 1 }),
            ("1", ExprType::Duration { max_ms: 1 }),
            ("1 < 2", ExprType::Bool),
            ("1 == 2", ExprType::Bool),
            ("1 != 2", ExprType::Bool),
            ("1 <= 2", ExprType::Bool),
            ("1 > 0", ExprType::Bool),
            ("1 >= 1", ExprType::Bool),
            ("!true", ExprType::Bool),
            ("-1", ExprType::Int),
            ("-1.5", ExprType::Number),
            ("-x", ExprType::Bool),
            ("x", ExprType::Bool),
            ("x.y", ExprType::Bool),
            ("x[0]", ExprType::Bool),
            ("f()", ExprType::Bool),
            ("x.f()", ExprType::Bool),
            ("1 + 2", ExprType::Number),
            ("1 - 2", ExprType::Int),
            ("2 * 3", ExprType::Int),
            ("4 / 2", ExprType::Int),
            ("5 % 2", ExprType::Int),
            ("'a' + 'b'", ExprType::String { max_bytes: 2 }),
        ];
        for (source, expected) in accepted {
            assert!(
                Expr::compile_typed(source, expected).is_ok(),
                "{source} as {:?}",
                expected
            );
        }
        assert!(Expr::compile_typed(&source, ExprType::String { max_bytes: 20_000 }).is_ok());

        assert!(matches!(
            assert_err(Expr::compile_typed("null", ExprType::Bool)),
            Error::ResultType {
                expected: "bool",
                got: "null"
            }
        ));
        assert!(matches!(
            assert_err(Expr::compile_typed("1", ExprType::String { max_bytes: 0 })),
            Error::ResultType {
                expected: "string",
                got: "int"
            }
        ));
        assert!(matches!(
            assert_err(Expr::compile_typed(
                "'abc'",
                ExprType::String { max_bytes: 2 }
            )),
            Error::TooLarge {
                what: "string result",
                ..
            }
        ));
        assert!(matches!(
            assert_err(Expr::compile_typed("1 + 'a'", ExprType::Number)),
            Error::ResultType { got: "other", .. }
        ));
        assert!(matches!(
            assert_err(Expr::compile_typed("1 - 1.0", ExprType::Number)),
            Error::ResultType { got: "other", .. }
        ));
    }

    #[test]
    fn expression_type_runtime_checks_every_arm() {
        assert!(ExprType::Any.check_value(&Value::Null).is_ok());
        assert!(ExprType::Bool.check_value(&Value::Bool(true)).is_ok());
        assert_eq!(
            ExprType::Bool.check_value(&Value::Int(1)).unwrap_err(),
            Error::ResultType {
                expected: "bool",
                got: "int"
            }
        );
        assert!(ExprType::Int.check_value(&Value::Int(1)).is_ok());
        assert_eq!(
            ExprType::Int.check_value(&Value::UInt(1)).unwrap_err(),
            Error::ResultType {
                expected: "int",
                got: "uint"
            }
        );
        assert!(ExprType::Number.check_value(&Value::Int(1)).is_ok());
        assert!(ExprType::Number.check_value(&Value::Float(1.5)).is_ok());
        assert_eq!(
            ExprType::Number.check_value(&Value::from("x")).unwrap_err(),
            Error::ResultType {
                expected: "number",
                got: "string"
            }
        );
        assert_eq!(
            ExprType::Number
                .check_value(&Value::Float(f64::NAN))
                .unwrap_err(),
            Error::NonFinite {
                operation: "result"
            }
        );
        assert!(
            ExprType::String { max_bytes: 2 }
                .check_value(&Value::from("ab"))
                .is_ok()
        );
        assert_eq!(
            ExprType::String { max_bytes: 2 }
                .check_value(&Value::from("abc"))
                .unwrap_err(),
            Error::TooLarge {
                what: "string result",
                limit: 2
            }
        );
        assert_eq!(
            ExprType::String { max_bytes: 2 }
                .check_value(&Value::Int(1))
                .unwrap_err(),
            Error::ResultType {
                expected: "string",
                got: "int"
            }
        );
        assert!(
            ExprType::Duration { max_ms: 10 }
                .check_value(&Value::Int(10))
                .is_ok()
        );
        assert_eq!(
            ExprType::Duration { max_ms: 10 }
                .check_value(&Value::Int(11))
                .unwrap_err(),
            Error::ValueLimit {
                what: "duration result (ms)",
                limit: 10
            }
        );
        assert_eq!(
            ExprType::Duration { max_ms: 10 }
                .check_value(&Value::from("x"))
                .unwrap_err(),
            Error::ResultType {
                expected: "duration",
                got: "string"
            }
        );
    }

    #[test]
    fn static_kind_names_and_all_node_shapes() {
        for (kind, name) in [
            (StaticKind::Null, "null"),
            (StaticKind::Bool, "bool"),
            (StaticKind::Int, "int"),
            (StaticKind::Float, "float"),
            (StaticKind::Str, "string"),
            (StaticKind::Other, "other"),
            (StaticKind::Unknown, "unknown"),
        ] {
            assert_eq!(kind.name(), name);
        }
        for source in [
            "true", "1", "1.5", "'a'", "1 < 2", "1 == 2", "1 != 2", "1 <= 2", "1 > 0", "1 >= 1",
            "!true", "-1", "-1.5", "x", "x.y", "x[0]", "f()", "x.f()", "1 + 2", "1 - 2", "2 * 3",
            "4 / 2", "5 % 2", "1 + 'a'",
        ] {
            let expr = Expr::compile(source).unwrap();
            let _ = expr.eval(&Context::new());
        }
    }

    #[test]
    fn compile_entry_points_and_function_spec_pure() {
        let limits = CompileLimits::default();
        let context = Context::new();

        let expr = Expr::compile_with("1", &limits).unwrap();
        assert_eq!(expr.eval(&context).unwrap(), Value::Int(1));
        let expr = Expr::compile_typed_with("1", ExprType::Int, &limits).unwrap();
        assert_eq!(expr.eval(&context).unwrap(), Value::Int(1));
        let expr = Expr::compile_with_profile("1", BudgetProfile::Gateway).unwrap();
        assert_eq!(expr.profile(), BudgetProfile::Gateway);
        let expr =
            Expr::compile_typed_with_profile("1", ExprType::Int, BudgetProfile::Query).unwrap();
        assert_eq!(expr.expected_type(), ExprType::Int);
        let expr = Expr::compile_typed_full("1", ExprType::Int, BudgetProfile::Assignment, &limits)
            .unwrap();
        assert_eq!(expr.profile(), BudgetProfile::Assignment);
        assert!(!expr.is_empty());

        let pure = FunctionSpec::pure();
        assert_eq!(pure.purity, FunctionPurity::Pure);
        assert!(pure.deterministic);
        assert_eq!(pure.max_calls, FunctionSpec::default().max_calls);
    }
}
