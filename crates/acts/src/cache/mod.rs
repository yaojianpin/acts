#[allow(clippy::module_inception)]
mod cache;
mod store;
#[cfg(test)]
mod tests;

pub use cache::Cache;
