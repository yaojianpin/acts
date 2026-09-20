//! External [`KvStore`](acts::KvStore) backends for the acts workflow engine.
//!
//! Each backend implements the [`KvStore`](acts::KvStore) trait defined by the
//! `acts` crate and can be handed to
//! [`EngineBuilder::set_store`] the same way as [`MemoryStore`](acts::MemoryStore):
//!
//! ```rust,ignore
//! use acts::Engine;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> acts::Result<()> {
//!     let store = acts_store::SqliteStore::open("data/acts.db").await?;
//!     let engine = Engine::builder()
//!         .set_store(Arc::new(store))
//!         .start()
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! - [`SqliteStore`] — feature `sqlite`
//! - [`PostgresStore`] — feature `postgres`
//! - [`SledStore`] — feature `sled`
//!
//! Every backend here commits a batch with its guards (see
//! [`KvStore::batch`](acts::KvStore::batch)), which is what
//! [`DbLease`](acts::DbLease) needs to hold a database exclusively across
//! processes.

mod consts;

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sled")]
mod sled;
#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
#[cfg(feature = "sled")]
pub use sled::SledStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;
