//! A small expression evaluator for the expressions a workflow carries.
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
//! `==` compares numbers across `int` and `float`, lists and objects
//! element-wise, and values of different kinds are not equal (so a workflow
//! that compares a missing value with a string is `false`, not an error).
//! Arithmetic requires numbers, `+` also concatenates two strings, and
//! `&& || !` require bools.
//!
//! # Injection
//!
//! Everything an expression reads is in the [`Context`]: scalars, lists and
//! objects ([`Value::Map`]) as values, and methods as [`Value::Function`]s.
//! A function is called as `f(x)`, or as a method with its receiver first —
//! `x.f()` — and a function stored *inside* an object is called that way
//! through the object, so an injected object carries its own methods.
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
//! (`[...]`, `{...}`), no `in`, no comprehension, no method on a built-in type
//! (there are no built-in types: a string is a string, and every method is
//! injected). Each of those is rejected with a message that says so.

mod error;
mod eval;
mod lexer;
mod parser;
mod value;

#[cfg(test)]
mod tests;

pub use error::{Error, Result};
pub use eval::Context;
pub use value::{Map, NativeFunction, Value};

use parser::Node;

/// A compiled expression.
///
/// Compiling is the only step that reads the source; evaluation is a walk over
/// the compiled nodes, so a host that evaluates the same expression per task —
/// a step condition, a parameter filled per process — compiles it once and
/// keeps the [`Expr`].
#[derive(Debug, Clone)]
pub struct Expr {
    nodes: Vec<Node>,
    root: u32,
}

impl Expr {
    /// Compile `source`, or return the byte offset the parse stopped at.
    pub fn compile(source: &str) -> Result<Expr> {
        let (nodes, root) = parser::Parser::new(source)?.parse()?;

        Ok(Expr { nodes, root })
    }

    /// Evaluate against `context`.
    pub fn eval(&self, context: &Context) -> Result<Value> {
        eval::eval(&self.nodes, self.root, context)
    }

    /// The names this expression reads, deduplicated and sorted: variables,
    /// called functions, and the names of called methods (which a receiver may
    /// answer itself before the context is consulted).
    ///
    /// A host that injects on demand — rather than filling a context with
    /// everything an expression *might* read — injects exactly these.
    pub fn references(&self) -> Vec<&str> {
        let mut names = Vec::new();

        for node in &self.nodes {
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
    /// what it accepts.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// Compile and evaluate in one step, for a caller that has no reason to keep
/// the compiled expression.
pub fn eval(source: &str, context: &Context) -> Result<Value> {
    Expr::compile(source)?.eval(context)
}
