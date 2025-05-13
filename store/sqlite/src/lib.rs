//! Acts sqlite store

#![allow(rustdoc::bare_urls)]
#![doc = include_str!("../README.md")]

mod collection;
mod database;

#[cfg(test)]
mod tests;

use acts::ActPlugin;

#[derive(Clone)]
pub struct SqliteStore;

impl ActPlugin for SqliteStore {
    fn on_init(&self, engine: &acts::Engine) {
        let config = engine.config();
        let Some(db_url) = &config.database_url else {
            panic!("cannot find database_url in config file");
        };

        let db = database::Database::new(db_url);
        db.init();

        engine.extender().register_collection(db.packages());
        engine.extender().register_collection(db.models());
        engine.extender().register_collection(db.procs());
        engine.extender().register_collection(db.tasks());
        engine.extender().register_collection(db.messages());
        engine.extender().register_collection(db.events());
    }
}
