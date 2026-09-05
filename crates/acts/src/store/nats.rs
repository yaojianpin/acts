use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions},
    utils::consts,
};
use async_nats::jetstream;
use futures::StreamExt;

pub struct NatsStore {
    kv: jetstream::kv::Store,
}

impl NatsStore {
    pub async fn open(url: &str) -> Result<Self> {
        let bucket = consts::ACTS_STORE_NAME;
        let client = async_nats::connect(url)
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
        Ok(Self { kv })
    }
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq => k.starts_with(key),
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
        ScanOperation::Range { lower, upper } => {
            if let Some(l) = lower
                && k < l.as_str()
            {
                return false;
            }
            if let Some(u) = upper
                && k >= u.as_str()
            {
                return false;
            }
            true
        }
    }
}

#[async_trait::async_trait]
impl KvStore for NatsStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.kv
            .get(key)
            .await
            .map(|entry| entry.map(|e| e.to_vec()))
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.kv
            .put(key, value.into())
            .await
            .map(|_| ())
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.kv
            .delete(key)
            .await
            .map_err(|e| ActError::Store(e.to_string()))
    }

    async fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions { is_rev, op, prefix } = options;
        let keys = self
            .kv
            .keys()
            .await
            .map_err(|e| ActError::Store(e.to_string()))?;
        futures::pin_mut!(keys);
        let mut result = Vec::new();
        while let Some(k) = keys.next().await {
            let k = k.map_err(|e| ActError::Store(e.to_string()))?;
            if key_matches(&k, key, &prefix, &op)
                && let Some(entry) = self
                    .kv
                    .get(&k)
                    .await
                    .map_err(|e| ActError::Store(e.to_string()))?
            {
                result.push((k, entry.to_vec()));
            }
        }
        if is_rev {
            result.reverse();
        }
        Ok(result)
    }
}
