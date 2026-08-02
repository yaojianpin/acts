//! Acts postgres store

#![allow(rustdoc::bare_urls)]
// #![doc = include_str!("../README.md")]

mod config;
mod package;

#[cfg(test)]
mod tests;

pub use package::StatePackage;
