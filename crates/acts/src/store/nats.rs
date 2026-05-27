use super::kv::KvStore;
use crate::{ActError, Result, utils::consts};
use async_nats::jetstream;
use futures::StreamExt;
use tokio::runtime::Runtime;

pub struct NatsStore {
    kv: jetstream::kv::Store,
    runtime: Runtime,
}

impl NatsStore {
    pub fn open(url: &str) -> Result<Self> {
        tokio::task::block_in_place(|| {
            let runtime = Runtime::new().map_err(|e| ActError::Store(e.to_string()))?;
            let url = url.to_string();
            let bucket = consts::ACTS_STORE_NAME;
            let kv = runtime.block_on(async {
                let client = async_nats::connect(&url)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                let jetstream = jetstream::new(client);
                let kv = match jetstream.get_key_value(bucket).await {
                    Ok(store) => store,
                    Err(_) => jetstream
                        .create_key_value(jetstream::kv::Config {
                            bucket: bucket.to_string(),
                            ..Default::default()
                        })
                        .await
                        .map_err(|e| ActError::Store(e.to_string()))?,
                };
                Ok::<_, ActError>(kv)
            })?;
            Ok(Self { kv, runtime })
        })
    }
}

impl KvStore for NatsStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                self.kv
                    .get(&key)
                    .await
                    .map(|entry| entry.map(|e| e.to_vec()))
                    .map_err(|e| ActError::Store(e.to_string()))
            })
        })
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let key = key.to_string();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                self.kv
                    .put(&key, value.into())
                    .await
                    .map(|_| ())
                    .map_err(|e| ActError::Store(e.to_string()))
            })
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                self.kv
                    .delete(&key)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))
            })
        })
    }

    fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let prefix = prefix.to_string();
        tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                let keys = self
                    .kv
                    .keys()
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?;
                futures::pin_mut!(keys);
                let mut result = Vec::new();
                while let Some(key) = keys.next().await {
                    let key = key.map_err(|e| ActError::Store(e.to_string()))?;
                    if key.starts_with(&prefix) {
                        if let Some(entry) = self
                            .kv
                            .get(&key)
                            .await
                            .map_err(|e| ActError::Store(e.to_string()))?
                        {
                            result.push((key, entry.to_vec()));
                        }
                    }
                }
                Ok(result)
            })
        })
    }
}
