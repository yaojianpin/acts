use crate::{store::StoreAdapter, Engine, ShareLock};
use std::sync::{Arc, RwLock};
use tracing::info;

#[cfg(test)]
mod tests;

pub fn init(_engine: &Engine) {}

#[derive(Clone)]
pub struct Adapter {
    store: ShareLock<Option<Arc<dyn StoreAdapter>>>,
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_store<STORE: StoreAdapter + Clone + 'static>(&self, store: &STORE) {
        info!("set_store");
        *self.store.write().unwrap() = Some(Arc::new(store.clone()));
    }

    pub fn store(&self) -> Option<Arc<dyn StoreAdapter>> {
        self.store.read().unwrap().clone()
    }
}
