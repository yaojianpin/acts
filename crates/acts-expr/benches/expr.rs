//! Compiling and evaluating the shapes a workflow's `${{ }}` placeholders
//! carry. Prints ns per operation instead of asserting on it — a bench target
//! is compiled by `cargo clippy --all-targets` but never run by CI, and a
//! shared runner's timings describe the runner.
//!
//! Run it with `cargo bench -p acts-expr` (add `--features json` to include the
//! JSON bridge in what is measured).

use acts_expr::{Context, Expr, Value};
use std::hint::black_box;
use std::time::Instant;

const EXPRS: [&str; 9] = [
    "(1 * 1000) + 1",
    "a + b",
    "a * 1000",
    "'step-' + name",
    "count > 10 ? 'big' : 'small'",
    "status == 'ok' && retries < 3",
    "secrets.TOKEN",
    "step1.value + 1",
    "$env.WORK_DIR",
];

/// The expressions above, with the ternary the evaluator does not have
/// rewritten to its equivalent — the bench measures the engine's shapes, not
/// the one form this crate deliberately leaves out.
fn benchable(source: &str) -> String {
    source
        .replace(
            "count > 10 ? 'big' : 'small'",
            "count > 10 && status == 'ok'",
        )
        .to_string()
}

fn context() -> Context {
    let mut context = Context::new();
    context
        .set("a", 10)
        .set("b", 32)
        .set("name", "alpha")
        .set("count", 42)
        .set("status", "ok")
        .set("retries", 1)
        .set("secrets", Value::map([("TOKEN", Value::from("sk-abc"))]))
        .set("step1", Value::map([("value", Value::from(7))]))
        .set("$env", Value::map([("WORK_DIR", Value::from("/tmp/run"))]));

    context
}

fn digest(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::Bool(b) => u64::from(*b),
        Value::Int(i) => *i as u64,
        Value::UInt(u) => *u,
        Value::Float(f) => *f as u64,
        Value::Str(s) => s.len() as u64,
        Value::List(items) => items.len() as u64,
        Value::Map(map) => map.len() as u64,
        Value::Function(_) => 0,
    }
}

fn main() {
    let iterations: u32 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(200_000);

    let context = context();
    let sources: Vec<String> = EXPRS.iter().map(|source| benchable(source)).collect();
    let programs: Vec<Expr> = sources
        .iter()
        .map(|source| Expr::compile(source).expect("compile"))
        .collect();

    // Warm up every shape, so the measurement is steady state.
    let mut checksum = 0u64;
    for (source, program) in sources.iter().zip(programs.iter()) {
        for _ in 0..(iterations / 10) {
            checksum = checksum.wrapping_add(digest(&black_box(program).eval(&context).unwrap()));
            checksum = checksum.wrapping_add(digest(
                &black_box(Expr::compile(source))
                    .unwrap()
                    .eval(&context)
                    .unwrap(),
            ));
        }
    }

    println!("acts-expr: {iterations} iterations per expression");
    let (mut total_compile, mut total_eval) = (0f64, 0f64);

    for (source, program) in sources.iter().zip(programs.iter()) {
        let start = Instant::now();
        for _ in 0..iterations {
            checksum = checksum.wrapping_add(digest(
                &black_box(Expr::compile(black_box(source)))
                    .unwrap()
                    .eval(&context)
                    .unwrap(),
            ));
        }
        let compile_eval = start.elapsed().as_nanos() as f64 / f64::from(iterations);

        let start = Instant::now();
        for _ in 0..iterations {
            checksum = checksum.wrapping_add(digest(
                &black_box(program).eval(black_box(&context)).unwrap(),
            ));
        }
        let eval = start.elapsed().as_nanos() as f64 / f64::from(iterations);

        total_compile += compile_eval;
        total_eval += eval;
        println!(
            "  {:>8.0} ns compile+eval  {:>7.0} ns eval  {}",
            compile_eval, eval, source
        );
    }

    let count = sources.len() as f64;
    println!(
        "  {:>8.0} ns compile+eval  {:>7.0} ns eval  average over {count} expressions",
        total_compile / count,
        total_eval / count
    );
    println!("  checksum {checksum}");
}
