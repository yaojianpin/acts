use crate::{store::StoreAdapter, Engine, ShareLock};
use core::fmt;
use std::sync::{Arc, RwLock};
use tracing::info;

#[cfg(test)]
mod tests;

pub fn init(_engine: &Engine) {}

#[derive(Clone)]
pub struct Adapter {
    store: ShareLock<Option<Arc<dyn StoreAdapter>>>,
}

impl fmt::Debug for Adapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Adapter").finish()
    }
}

impl Default for Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_store(&self, store: Arc<dyn StoreAdapter>) {
        info!("set_store");
        *self.store.write().unwrap() = Some(store);
    }

    pub fn store(&self) -> Option<Arc<dyn StoreAdapter>> {
        self.store.read().unwrap().clone()
    }
}
