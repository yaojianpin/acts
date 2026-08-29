# Installation

`acts` is a fast, lightweight, extensible workflow engine library that executes workflows defined in YAML format with a message-driven architecture.

## Install acts Library

Install via `cargo`:

```bash
cargo add acts
```

## External Storage

The following storage backends are available:

```bash
# SQLite
cargo add acts --features store-sqlite

# PostgreSQL
cargo add acts --features store-postgres

# NATS
cargo add acts --features store-nats

## Select the Store Backend

The features only decide which backend structs are compiled and exported;
create the backend externally and pass it to `EngineBuilder::set_store`
(the default is an in-memory store):

```rust
use acts::{Engine, store::SqliteStore};
use std::sync::Arc;

let engine = Engine::builder()
    .set_store(Arc::new(SqliteStore::open("data/acts.db").unwrap()))
    .build()
    .start()
    .unwrap();
```

When the matching feature is enabled, `SqliteStore`, `PostgresStore`,
`RedisStore`, `NatsStore` and `SledStore` are exported from the crate.

## Create Engine

```rust
use acts::Engine;

let engine = Engine::new().start().unwrap();
let executor = engine.executor();
```

## Deploy and Start Workflow

```rust
use acts::{Engine, Vars, Workflow};

let engine = Engine::new().start().unwrap();

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
