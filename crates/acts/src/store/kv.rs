use crate::Result;

/// Key-value store trait.
///
/// All methods are synchronous. Async backends should
/// internally use `tokio::runtime::Handle::current().block_on()`.
pub trait KvStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: Vec<u8>) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;
}
