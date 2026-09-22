//! The `acts.app.javascript` package: runs JavaScript code with an embedded
//! QuickJS runtime.
//!
//! The engine's own `${{ }}` expression evaluator is `acts-expr` (see the
//! `acts` crate); QuickJS lives here, behind the `acts.app.javascript` package that
//! executes arbitrary JavaScript for variable computation and transformation.

#![allow(rustdoc::bare_urls)]

mod code;
mod env;
mod module;
mod value;

#[cfg(test)]
mod tests;

pub use code::CodePackage;
