# Installation

`acts` is a fast, lightweight, extensible workflow engine library that executes workflows defined in YAML format with a message-driven architecture.

## Install acts Library

Install via `cargo`:

```bash
cargo add acts
```

## External Storage

The persistent storage backends (sqlite/postgres/redis/nats/sled) live in the
`acts-store` crate — enable the matching feature and import the backend from
`acts_store`, then pass it to `EngineBuilder::set_store` (when unset, an
in-memory `MemoryStore` is used):

```bash
# SQLite
cargo add acts-store --features sqlite

# PostgreSQL
cargo add acts-store --features postgres

# NATS
cargo add acts-store --features nats

# Redis
cargo add acts-store --features redis

# Sled
cargo add acts-store --features sled
```

```rust
use acts::Engine;
use acts_store::SqliteStore; // or PostgresStore / RedisStore / NatsStore / SledStore
use std::sync::Arc;

#[tokio::main]
async fn main() -> acts::Result<()> {
    let store = SqliteStore::open("data/acts.db").await?;
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .start()
        .await?;
    Ok(())
}
```

`acts` itself ships only `MemoryStore` and the `KvStore` trait custom stores
implement; a custom backend implementing `acts::KvStore` is injected the same
way via `set_store`.

Each database has **one writer**. The engine's document locks are process-local
— every engine in one process shares one lock table (two databases that happen
to use the same keys are serialized together, which costs contention and
nothing else) — and they do **not** cross processes, so two processes writing
one database have no mutual exclusion: their concurrent updates of a row can
leave the index entries disagreeing with the data row, a query matching a value
the row no longer holds or missing the one it does. A single-instance
deployment is unaffected; a deployment with several gives each its own
database, or coordinates outside the engine (a backend conditional write, or a
lock spanning the read) — `batch` makes one write atomic, not a read plus
another process's write.

## Create Engine

```rust
use acts::Engine;

let engine = Engine::builder().start().await.unwrap();
let executor = engine.executor();
```

## Deploy and Start Workflow

```rust
use acts::{Engine, Vars, Workflow};

let engine = Engine::builder().start().await.unwrap();

// Load YAML model
let model = r#"
id: my_model
name: my model
steps:
  - name: step 1
    uses: acts.transform.set
    params:
      a: 10
  - name: step 2
    uses: acts.transform.code
    params: |
      return { data: a + 10 };
"#;
let workflow = Workflow::from_yml(model).unwrap();

// Deploy model
let executor = engine.executor();
executor.model().deploy(&workflow).expect("fail to deploy workflow");

// Start workflow
let mut vars = Vars::new();
vars.set("a", 0);
vars.set("pid", "w1");
executor.proc().start(&workflow.id, vars).expect("fail to start workflow");
```

## Related Projects

| Project | Description |
| ---- | ---- |
| [acts-server](https://github.com/yaojianpin/acts-server) | gRPC-based workflow service |
| [acts-channel](https://github.com/yaojianpin/acts-channel) | Rust client library |
| [acts-channel-py](https://github.com/yaojianpin/acts-channel-py) | Python client library |
| [acts-channel-go](https://github.com/yaojianpin/acts-channel-go) | Go client library |
