//! Acts postgres store

#![allow(rustdoc::bare_urls)]
#![doc = include_str!("../README.md")]

mod collection;
mod database;
mod synclient;

#[cfg(test)]
mod tests;

use acts::{ActError, ActPlugin, Result};

#[derive(Clone)]
pub struct PostgresStore;

#[derive(serde::Deserialize)]
struct ProgresConfig {
    database_url: String,
}

#[async_trait::async_trait]
impl ActPlugin for PostgresStore {
    async fn on_init(&self, engine: &acts::Engine) -> Result<()> {
        let config = engine
            .config()
            .get::<ProgresConfig>("postgres")
            .map_err(|err| ActError::Config(format!("get postgres config error: {}", err)))?;

        let db = database::Database::new(&config.database_url);
        db.init();

        engine.extender().register_collection(db.packages());
        engine.extender().register_collection(db.models());
        engine.extender().register_collection(db.procs());
        engine.extender().register_collection(db.tasks());
        engine.extender().register_collection(db.messages());
        engine.extender().register_collection(db.events());

        Ok(())
    }
}
