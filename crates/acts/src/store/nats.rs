use crate::{
    ActError, KvStore, Result,
    store::{ScanOperation, ScanOptions},
    utils::{consts, sync},
};
use async_nats::jetstream;
use futures::StreamExt;

pub struct NatsStore {
    kv: jetstream::kv::Store,
}

impl NatsStore {
    pub fn open(url: &str) -> Result<Self> {
        let url = url.to_string();
        sync::block_on(async move {
            let bucket = consts::ACTS_STORE_NAME;
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
            Ok(Self { kv })
        })
    }
}

/// Return true if `k` matches the scan operation given `key` and `prefix`.
fn key_matches(k: &str, key: &str, prefix: &str, op: &ScanOperation) -> bool {
    if !k.starts_with(prefix) {
        return false;
    }
    match op {
        ScanOperation::Eq | ScanOperation::Match => k.starts_with(key),
        ScanOperation::Gt => k > key,
        ScanOperation::Ge => k >= key,
        ScanOperation::Lt => k < key,
        ScanOperation::Le => k <= key,
        ScanOperation::Ne => !k.starts_with(key),
        ScanOperation::Range { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k >= start.as_str() && k < end.as_str()
        }
        ScanOperation::ExclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k > start.as_str() && k < end.as_str()
        }
        ScanOperation::InclusiveRange { from, to } => {
            let start = format!("{}{}", key, from);
            let end = format!("{}{}", key, to);
            k >= start.as_str() && k <= end.as_str()
        }
        ScanOperation::In { values } => values.iter().any(|v| k.starts_with(v.as_str())),
    }
}

impl KvStore for NatsStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = key.to_string();
        let kv = self.kv.clone();
        sync::block_on(async move {
            kv.get(&key)
                .await
                .map(|entry| entry.map(|e| e.to_vec()))
                .map_err(|e| ActError::Store(e.to_string()))
        })
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        let key = key.to_string();
        let kv = self.kv.clone();
        sync::block_on(async move {
            kv.put(&key, value.into())
                .await
                .map(|_| ())
                .map_err(|e| ActError::Store(e.to_string()))
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        let kv = self.kv.clone();
        sync::block_on(async move {
            kv.delete(&key)
                .await
                .map_err(|e| ActError::Store(e.to_string()))
        })
    }

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        let ScanOptions {
            is_rev,
            op,
            ref prefix,
        } = options;
        let prefix = prefix.clone();
        let key = key.to_string();
        let kv = self.kv.clone();
        sync::block_on(async move {
            let keys = kv
                .keys()
                .await
                .map_err(|e| ActError::Store(e.to_string()))?;
            futures::pin_mut!(keys);
            let mut result = Vec::new();
            while let Some(k) = keys.next().await {
                let k = k.map_err(|e| ActError::Store(e.to_string()))?;
                if key_matches(&k, &key, &prefix, &op)
                    && let Some(entry) = kv
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
        })
    }
}
