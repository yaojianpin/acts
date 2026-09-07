//! External [`KvStore`](acts::KvStore) backends for the acts workflow engine.
//!
//! Each backend implements the [`KvStore`](acts::KvStore) trait defined by the
//! `acts` crate and can be handed to
//! [`EngineBuilder::set_store`](acts::Engine::builder) the same way as
//! [`MemoryStore`](acts::MemoryStore):
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
//!         .build()
//!         .start()
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! - [`SqliteStore`] — feature `sqlite`
//! - [`PostgresStore`] — feature `postgres`
//! - [`RedisStore`] — feature `redis`
//! - [`NatsStore`] — feature `nats`
//! - [`SledStore`] — feature `sled`

mod consts;

#[cfg(feature = "nats")]
mod nats;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "redis")]
mod redis;
#[cfg(feature = "sled")]
mod sled;
#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "nats")]
pub use nats::NatsStore;
#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
#[cfg(feature = "redis")]
pub use redis::RedisStore;
#[cfg(feature = "sled")]
pub use sled::SledStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;
