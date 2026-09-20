# Installation

`acts` is a fast, lightweight, extensible workflow engine library that executes workflows defined in YAML format with a message-driven architecture.

## Install acts Library

Install via `cargo`:

```bash
cargo add acts
```

Building from source needs Rust 1.88 or newer: it is the floor every crate
promises (`rust-version` in each manifest, inherited from the workspace) and
what the current dependency graph already requires. CI builds that floor
(`1.88.0`) beside `stable`; `rust-toolchain.toml` selects the stable channel for
a checkout.

## External Storage

The persistent storage backends (sqlite/postgres/sled) live in the `acts-store`
crate — enable the matching feature and import the backend from `acts_store`,
then pass it to `EngineBuilder::set_store` (when unset, an in-memory
`MemoryStore` is used):

```bash
# SQLite
cargo add acts-store --features sqlite

# PostgreSQL
cargo add acts-store --features postgres

# Sled
cargo add acts-store --features sled
```

```rust
use acts::Engine;
use acts_store::SqliteStore; // or PostgresStore / SledStore
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

Each database has **one writer**, and the engine now enforces it: `acts-server`
takes an exclusive **database lease** before it starts an engine
(`[db] lease = true`, the default). The lease is a row in the database itself
(`__acts_lease__`), claimed with an atomic compare-and-swap, renewed every
`[db] lease_renew_secs`, and every write the engine makes is committed only
while the instance still holds it:

- a second server on the same database fails its startup with `LeaseHeld`,
  naming the instance that holds it, instead of starting a second engine that
  would recover and schedule the same rows;
- an instance whose lease is taken over — its renewals failed for longer than
  `[db] lease_ttl_secs`, so another instance claimed it — has every write
  refused with `LeaseLost` and its engine stopped, so it can never overwrite
  the new holder's rows;
- the holder's **fence** rises with every acquisition, across crashes and
  restarts, and a write carrying a stale fence is refused *inside the same
  atomic write that would have committed it*, not by a check that another
  process can race.

This is what closes the gap the engine's own locks cannot: the document locks
(`DOC_LOCKS`) that keep a document row and its index rows consistent are
per-process — every engine in one process shares one table, but two processes
each hold a private one — so without the lease two processes could both read a
row, both compute the index rows to write, and both commit, leaving a query
matching a value the row no longer holds.

The lease is built on one primitive: `KvStore::batch(ops, guards)` commits the
ops — and only while every guard still holds — as one atomic step of the
backend. The guard is the stored state the caller decided on, re-checked where
the write lands rather than before it, so a second writer cannot slip between
the read and the write. `Ok(false)` means a guard moved: nothing was applied.
Every backend in `acts-store` (sqlite/postgres/sled) and the in-memory
`MemoryStore` implement it; a custom `KvStore` that does not inherits a
refusal instead of a silent check-then-write.

`lease = false` is how an embedder or a deployment keeps the older contract —
**one writer per database is then yours to guarantee**, by giving each server
its own database or coordinating outside the engine. An embedded
`EngineBuilder::set_store` (the docs above) never takes a lease either: it is
the raw store you passed, so the single-writer rule is yours there too. The
`acts-server` binary is the path that manages the lease for you.

## Create Engine

```rust
use acts::{Engine, Principal};

let engine = Engine::builder().start().await.unwrap();
// The executor acts for a caller; an embedder driving the engine itself is
// the engine's own principal (see the access-control chapter).
let executor = engine.executor(&Principal::unrestricted());
```

## Deploy and Start Workflow

```rust
use acts::{Engine, Principal, Vars, Workflow};

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
let executor = engine.executor(&Principal::unrestricted());
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
