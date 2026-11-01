# Acts workflow engine

[![Build](https://github.com/yaojianpin/acts/actions/workflows/rust.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=rust)
[![Test](https://github.com/yaojianpin/acts/actions/workflows/test.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=test)
[![Coverage](https://raw.githubusercontent.com/yaojianpin/acts/badges/coverage.svg)](https://github.com/yaojianpin/acts/actions/workflows/coverage.yml)
[![Stress](https://raw.githubusercontent.com/yaojianpin/acts/stress/stress.svg)](https://github.com/yaojianpin/acts/actions/workflows/stress.yml)
[![Prof](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-memory.svg)](https://github.com/yaojianpin/acts/actions/workflows/prof.yml)

Acts is a fast, lightweight, extensible workflow engine for workflows defined in YAML. It runs as an embedded Rust library or as a server, and it is durable by design: every run is a transactional aggregate, every state transition commits together with its outbox records, and an interrupted engine resumes exactly where its aggregate left off.

Workflows are built from steps and branches. A step that declares a package (`uses`) performs the action; branch conditions select the path; joins, subflows, timers, human acts and message acts are part of the same kernel. The engine is message-driven: a run advances by claiming an operation, applying one deterministic transition, and committing the resulting aggregate and derived indexes — never by mutating an in-memory process object.

## Architecture

`Engine::builder().start()` returns an `Engine` that owns one runtime and one kernel host. The runtime is layered:

- **Kernel (`scheduler`)** — operations, claims, transition planning, outbox records and the run lifecycle (start, terminal, reclaim). One operation is one guarded commit: the aggregate, its appended log entry and the derived indexes move together or not at all.
- **Durable layer (`scheduler/durable`)** — each run is an aggregate family (head, base snapshot, append-only log window and recovery/outbox/ready indexes) in a `KvStore`. Reads take one consistent snapshot of the family; commits are compare-and-swap batches; the log compacts online. `MemoryStore` is the default; `acts-store` adds sled, SQLite and Postgres, each with native snapshot reads.
- **Runtime** — pid-keyed scheduler lanes, store-writer shards, and independent ready, recovery, outbox, cleanup and projection loops with bounded batches and their own health. In-memory queues are bounded and apply backpressure; the durable indexes are the overflow.
- **Read model** — `procs`, `tasks` and `vars` are a versioned projection of the aggregate. Every row records the revision it was derived from and is written conditionally, so a slow projection cannot overwrite a newer one; `proc:get` re-projects a lagging run before answering.

A started run is one aggregate; a client action (`act:*`, `proc:*`) is re-planned against the aggregate loaded from the store. Recovery scans the same durable indexes, the outbox delivers at least once with claims and quarantine, and terminal cleanup reclaims the aggregate, its rows and its workdir. `cache_cap` bounds runs in flight: a start beyond it is parked durably (state `None`) and admitted when a slot frees.

## Key Features

### Fast

- One operation, one commit: claim, plan, effect and derived indexes are one guarded batch.
- Consistent snapshot loads, a bounded decode cache, and an append-only log with online compaction.
- Pid-keyed scheduler lanes and store-writer shards run independent processes in parallel while preserving per-process order.
- Bounded in-memory queues with a durable overflow, so overload slows intake instead of losing work.
- Continuous measurement instead of a static table: the [performance and stress gates](#performance-and-stress-gates) publish throughput, latency and resource trends for both stores.

### Lightweight

- A single Rust library with no virtual machine and no mandatory broker; `Engine::builder().start()` is the whole setup.
- The default store is in-memory; durability is a backend choice (sled, SQLite, Postgres) behind the same `KvStore`.
- One kernel host per engine with independent loops, bounded batches, and no resident object per completed run: finished runs are reclaimed.
- Optional layers — server, CLI, channels, packages and plugins — are separate crates; the core does not require them.

### Extensible

- **Stores** — implement `KvStore` (see `crates/acts-store/src`) or use the embedded and SQL backends.
- **Packages** — declare `uses` in YAML and implement effects in Rust, JavaScript, HTTP, shell or state; see `examples/custom_package` and `packages/`.
- **Plugins and channels** — gRPC, web and NATS integrations under `plugins/`, with delivery and transport seams in the kernel.
- **Control** — roles and principals via `acts-acl`, snapshots and sealed values via the snapshot manager, expressions via `acts-expr`.

## Performance and stress gates

[![Stress](https://raw.githubusercontent.com/yaojianpin/acts/stress/stress.svg)](https://github.com/yaojianpin/acts/blob/stress/report.md)
[![Throughput](https://raw.githubusercontent.com/yaojianpin/acts/stress/stress-throughput.svg)](https://github.com/yaojianpin/acts/blob/stress/report.md)
[![Peak RSS](https://raw.githubusercontent.com/yaojianpin/acts/stress/stress-rss.svg)](https://github.com/yaojianpin/acts/blob/stress/report.md)
[![Rows left](https://raw.githubusercontent.com/yaojianpin/acts/stress/stress-rows.svg)](https://github.com/yaojianpin/acts/blob/stress/report.md)

[`examples/stress`](examples/stress) is the load generator behind these numbers. The [`stress` workflow](.github/workflows/stress.yml) runs it on every pull request that touches the scheduler, the store, or the harness — a 1,000-workflow performance smoke at concurrency 16 — and on `main`/`develop`, weekly, and by manual dispatch with 5,000 workflows across concurrency 16/32/64 and a cleanup drain wait.

The gate is deterministic: every measured workflow completes, the error rate is zero, no phase times out, background health never degrades, and after the drain the process-row count, residual aggregate documents, and residual index documents are zero. Throughput and peak RSS are published as a trend — a shared runner's timings move by double-digit percents between runs, so they are not a regression threshold.

The latest [`report.md`](https://github.com/yaojianpin/acts/blob/stress/report.md) (with `report.json`, `metrics.csv`, and `trace.log`) is published to the `stress` branch, and every run also attaches them as the `stress-report` artifact. Manual dispatch can choose the workflow count, concurrency stages, cleanup wait, soak, and store backend (memory by default; sled/sqlite for the durable paths).

Reference run (memory store, 8-core workstation, 10,000 workflows at c16/32/64): **82.4 workflows/s**, **0.24 GiB peak RSS**, **0 rows / 0 aggregate documents / 0 index documents after the cleanup wait**, 0 degraded health samples.

### Completion microbenchmark (`examples/prof`)

[`examples/prof`](examples/prof) is a completion-latency microbenchmark, not a load test: it parks N processes on one human act and times each `complete()` after a warmup. `--store memory` measures the in-memory engine path; `--store sled` measures the same path with the durable sled backend, so the two numbers are published separately — one never stands in for the other.

| Store | Status | Avg | p99 | Throughput |
| --- | --- | --- | --- | --- |
| memory | [![prof memory](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-memory.svg)](https://github.com/yaojianpin/acts/blob/prof/report-memory.md) | [![avg memory](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-memory-avg.svg)](https://github.com/yaojianpin/acts/blob/prof/report-memory.md) | [![p99 memory](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-memory-p99.svg)](https://github.com/yaojianpin/acts/blob/prof/report-memory.md) | [![rate memory](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-memory-throughput.svg)](https://github.com/yaojianpin/acts/blob/prof/report-memory.md) |
| sled | [![prof sled](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-sled.svg)](https://github.com/yaojianpin/acts/blob/prof/report-sled.md) | [![avg sled](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-sled-avg.svg)](https://github.com/yaojianpin/acts/blob/prof/report-sled.md) | [![p99 sled](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-sled-p99.svg)](https://github.com/yaojianpin/acts/blob/prof/report-sled.md) | [![rate sled](https://raw.githubusercontent.com/yaojianpin/acts/prof/prof-sled-throughput.svg)](https://github.com/yaojianpin/acts/blob/prof/report-sled.md) |

The [`prof` workflow](.github/workflows/prof.yml) runs both stores on pull requests (200/100 completions) and on `main`/`develop`, weekly, and manual dispatch (2,000/500), publishing `report-memory.*` and `report-sled.*` plus the badges to the `prof` branch. The gate is zero failed completions; latency and throughput are trend data.

```sh
cargo run --release -p acts-example-prof -- --store memory --iters 2000 --warmup 20 \
  --json report-memory.json --markdown report-memory.md
cargo run --release -p acts-example-prof -- --store sled --store-url target/prof-sled \
  --iters 500 --warmup 20 --json report-sled.json --markdown report-sled.md
```

## Installation

The easiest way to get the latest version of `acts` is to install it via `cargo`

```bash
cargo add acts
```

Building from source needs Rust 1.88 or newer: it is the floor every crate
promises (`rust-version` in each manifest, inherited from the workspace) and
what the current dependency graph already requires. CI builds that floor
(`1.88.0`) beside `stable`, and `rust-toolchain.toml` selects the stable channel
for a checkout.

## Documents
[`Chinese`](https://yaojianpin.github.io/acts/zh/)
[`English`](https://yaojianpin.github.io/acts/en/)

## Quickstart

1. Create and start the workflow engine with `Engine::builder().start()`.
2. Load a yaml model to create a `workflow`.
3. Deploy the model in step 2 by `engine.executor(&principal).model()`.
4. Config events by `engine.channel()`.
5. Start the workflow by `engine.executor(&principal).proc()`.

```rust,no_run
use acts::{Engine, Principal, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::builder().start().await.unwrap();

    // create yaml workflow model
    let model = r#"
    id: my_model
    name: my model
    steps:
      - name: step 1
        uses: acts.transform.set
        params:
            a: 10
      - name: step 2
        uses: acts.app.javascript
        params: |
            return { data: a + 10 };
    "#;
    let workflow = Workflow::from_yml(model).unwrap();

    // The executor acts for a caller: this one is the engine's own, which is
    // what an embedder that drives the engine itself passes. `Principal` is
    // also what a logged-in user's session token resolves to (see the
    // access-control chapter).
    let executor = engine.executor(&Principal::unrestricted());
    executor
        .model()
        .deploy(&workflow, None)
        .await
        .expect("fail to deploy workflow");

    let mut vars = Vars::new();

    // set the input value
    vars.set("a", 0);

    // set the pid or auto generate by engine
    vars.set("pid", "w1");

    // create channel to receive messages
    let chan = engine.channel();

    chan.on_start(|e| async move {
        println!("start: {}", e.start_time);
    });

    chan.on_message(|e| async move {
        println!("message: {:?}", e);
    });

    chan.on_complete(|e| async move {
        println!("outputs: {:?} end_time: {}", e.outputs, e.end_time);
    });

    chan.on_error(|e| async move {
        println!("error on proc id: {} model id: {}", e.pid, e.mid);
    });

    // start workflow by model id
    executor
        .proc()
        .start(&workflow.id, vars)
        .await
        .expect("fail to start workflow");
}
```

## Access control

The executor answers for a caller, and every operation is decided through that
principal. The policy types live in this crate: `Principal` is the compiled
answer, `AccessControl` is the port the engine talks through (`Engine::acl()`
hands back the installed one), and `UserPolicy` — compiled by
`Principal::from_policy` — is the plain-data form of a user's grants.

The shipped user registry is a crate of its own, `acts-acl`: `UserAcl` keeps its
users and its login sessions in the engine's store, and installing it is one
call on the builder, through the `AclUsers` trait it adds:

```text
use acts::Engine;
use acts_acl::AclUsers;

let engine = Engine::builder().with_user_acl().start().await?;
```

(`acts` does not depend on `acts-acl` — not even for tests — so that snippet is
not a doctest here.) A bare `Engine::builder().start()` runs `AnonymousAcl`: no
users, no login, and the anonymous catalogue-only policy for every caller.
`EngineBuilder::disable_acl()` is the explicit opt-out, where every caller is
unrestricted. See the access-control chapter of the book for the actions, the
grant groups, the sessions and the CLI that drives the registry.

## Examples

Please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

## Model Usage

The model is a yaml format file. where there are different type of node, including [`Workflow`], [`Branch`], [`Step`]. 


```yml
name: model name
# workflow default inputs vars
vars:
  - name: value
    value: 0

# schema for inputs and outputs
inputs:
  - name: value
    title: Value
    desc:  Set value when starting workflow
    type: number

# triggers to start the workflow
on:
  - id: event1
    kind: manual
# workflow steps
steps:
  - name: step 1
    # init with interrupt request to client
    # and make sure complete the action with 'list' var
    uses: acts.core.irq

  - name: step 2
    # workflow branches to run by condition
    branches:
      - name: branch 1
        if: value > 100
        steps:
          - name: step 3
            uses: acts.core.msg

      - name: branch 2
        if: value <= 100
        steps:
          - name: step 4
            uses: acts.core.parallel
            params:
              in: ${{ list }}
              acts:
                - uses: acts.core.irq
  - name: final step

```

### Vars

In the [`Workflow`], you can set the `vars` to init the workflow vars.

```yml
name: model name
vars:
  - name: a
    value: 100

steps:
  - name: step1
    uses: acts.app.javascript
    params: |
      // get the a variable
      let v = a + 100;
      // do somthing else
```

The vars can also be set by starting the workflow.

```rust,no_run
use acts::{Engine, Principal, Vars, Workflow};

#[tokio::main]
async fn main() {
  let engine = Engine::builder().start().await.unwrap();
  let executor = engine.executor(&Principal::unrestricted());

  let mut vars = Vars::new();
  vars.set("input", 3);
  vars.set("pid", "w2");

  executor.proc().start("m1", vars).await.unwrap();
}
```

### Options

In the [`Workflow`], you can set the `exposes` to filter the outputs.

```yml
name: model name
exposes:
  - name: output_key
steps:
  - name: step1
    uses: acts.transform.set
    params:
      output_key: 100
```


### Steps

Use `steps` to add step to the workflow

```yml
name: model name
steps:
  - id: step1
    name: step 1
  - id: step2
    name: step 2
```

For more acts example, please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

#### step.catches

Use the `catches` to capture the `step` error.

```yml
name: a catches example
id: catches
steps:
  - name: prepare
    id: prepare
    uses: acts.core.irq
  - name: step1
    id: step1
    uses: acts.core.irq

    # catch the step errors
    catches:
      - name: catch step 1
        id: catch1
        uses: acts.core.irq
        if: $ecode() == "err1"

      - name: catch step 2
        id: catch2
        uses: acts.core.irq
        if: $ecode() == "err2"

  - name: final
    id: final
```

#### step.timeouts

Use the `timeouts` to check the task time.

```yml
name: a timeout example
id: timeout
steps:
  - name: prepare
    id: prepare
    uses: acts.core.irq

  - name: step1
    id: step1
    uses: acts.core.irq

    # check timeout rules
    timeouts:
      # 1d means one day
      # triggers act2 when timeout
      - uses: acts.core.irq
        id: act2
        if: $cost_in('1d')

      # 2h means two hours
      # triggers act3 when timeout
      - uses: acts.core.irq
        id: act2
        if: $cost_in('2h')

  - name: final
    id: final
```

### Branches

Use `branches` to add branch to the step

```yml
name: model name
steps:
  - id: step1
    name: step 1
    branches:
      - id: b1
        if: v > 0
        steps:
          - name: step a
          - name: step b
      - id: b2
        else: true
        steps:
          - name: step c
          - name: step d
  - id: step2
    name: step 2
```

### Join

A step whose branches (or whose `acts.core.block{ mode: parallel }` /
`acts.core.parallel` acts) run together decides by one `join`: how many of the
children must complete before the step continues.

```yml
name: model name
steps:
  # any one vote completes the step; the votes still out are withdrawn
  - id: sign
    uses: acts.core.block
    params:
      mode: parallel
      join: any
      steps:
        - uses: acts.core.irq
          params:
            key: vote1
        - uses: acts.core.irq
          params:
            key: vote2

  # the first 2 of N items complete the step (partial(40%) works the same way)
  - id: countersign
    uses: acts.core.parallel
    params:
      in: ${{ approvers }}
      join: partial(2)
      steps:
        - uses: acts.core.irq
          params:
            key: vote

  # the client decides: one acts.core.decide request per settlement, answered
  # complete (pass) / fail (reject) / skip (defer)
  - id: board
    uses: acts.core.block
    params:
      mode: parallel
      join:
        action: countersign
        params:
          chair: chair
      steps:
        - uses: acts.core.irq
          params:
            key: vote
```

The join is `all` by default (every child must complete — a child failure
fails the step, the engine's original shape). `any` and `partial(...)` count
only the children really in the vote: a child its own guard skipped or the
client removed was never in it, and a failed vote is a vote against — the
container fails only when the threshold can no longer be reached
(`partial(n)` with `n` above the real children fails at the fan-out). A join
on a sequential shape (`acts.core.sequence`, `block{ mode: sequence }`) is
refused at start.

### Step rules

Use `rules` to decide what a step does when its act starts. Each rule's `if`
is a condition over the step's variables and its resolved params; the first
condition that holds runs its `action`:

- `error` (the default) fails the step before the act reaches a client, with
  `error.code = "validation_failed"` and the rule's own message — a `catches`
  handler picks it up like any other failure.
- `skip` drops the step and lets the flow continue (a concurrent-post approval
  where the same person holds both posts).
- `next`/`submit` start the act and apply the action as its first decision —
  automatic approval, with no `Created` request to a client; `abort`/`remove`
  close it the same way. `back`, `cancel` and `push` need a target or `uses` a
  rule does not carry and are refused.

A condition that does not hold moves on to the next rule; when no rule fires,
the act starts normally. A condition that cannot be evaluated always fails the
step. The conditions run on the engine's environment, so `$parent` (the
previous flow execution's data) and `$root` (the workflow root's data) are
available alongside the other built-ins.

```yml
name: model name
steps:
  - id: review
    uses: acts.core.irq
    vars:
      - { name: amount, value: 0 }
      - { name: approver, value: u1 }
    params:
      key: review
    rules:
      - if: "amount <= 0"       # when it holds, the step fails
        message: amount must be positive
      - if: "$parent.approver == $root.director"
        action: skip            # concurrent post: drop this approval
      - if: "$root.amount <= $root.limit"
        action: next            # under the limit: approve automatically
      - "amount > 10000"        # shorthand: no custom message
```

For more acts example, please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

The active backend is created externally and passed to
`EngineBuilder::set_store` — when unset, an in-memory store is used. The
persistent backends live in the `acts-store` crate:

```bash
cargo add acts-store --features sqlite
```

```rust,ignore
use acts::Engine;
use acts_store::SqliteStore; // or PostgresStore / SledStore
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let store = SqliteStore::open("data/acts.db").await.unwrap();
    let engine = Engine::builder()
        .set_store(Arc::new(store))
        .start()
        .await
        .unwrap();
}
```

Backends enabled by the matching `acts-store` feature:

- `MemoryStore` — built into `acts`, in-memory store, no persistence (default when unset)
- `SqliteStore` — `acts-store` feature `sqlite`
- `PostgresStore` — `acts-store` feature `postgres`
- `SledStore` — `acts-store` feature `sled`

SQLite and Postgres make each commit durable before it returns. `SledStore`
applies writes immediately and flushes in the background every 50 ms, so a
crash can lose the writes applied since the last completed flush; call
`KvStore::flush()` for an explicit barrier (a graceful engine shutdown does).

Custom stores can be built by implementing `acts::KvStore` (including its
`batch` with the guards in the same atomic unit — an exclusive database lease
depends on it) and passed to `set_store` the same way.

## Package

Please see `examples/custom_package`.

## Acts-Server

Create a acts-server to interact with clients based on grpc.
please see more from [`crates/acts-server`](https://github.com/yaojianpin/acts/tree/main/crates/acts-server)

## Client channels

- rust https://github.com/yaojianpin/acts-channel
- python https://github.com/yaojianpin/acts-channel-py
- go https://github.com/yaojianpin/acts-channel-go

## Roadmap

acts:

- runtime

  - [x] model (Workflow, Branch, Step)
  - [x] kernel scheduler (durable aggregates, outbox, recovery, row projection)
  - [x] expr expression
  - [x] in-flight run cap (`cache_cap`)
  - [x] plugin register
  - [x] package register
  - [x] snapshot register
  - [x] message channel

- triggers
  - [x] manual
  - [x] hook
  - [x] chat
  - [x] schedule

- store
  - [x] memory
  - [x] sqlite
  - [x] postgres
  - [x] sled

- packages

  - core
    - [x] irq
    - [x] msg
    - [x] block
    - [x] action
    - [x] parallel
    - [x] sequence
    - [x] subflow
    - [x] join
    - [x] candidates

  - transform
    - [x] set

  - app
    - [ ] form (plugins/form)
    - [ ] ai (plugins/ai)
    - [x] state (packages/acts-package-state)
    - [x] http (packages/acts-package-http)
    - [x] javascript (packages/acts-package-javascript)
    - [x] shell (packages/acts-package-shell) runs a bash script in bashkit's virtual filesystem
    - [x] pubsub (packages/acts-package-nats)
    - [ ] database (plugins/database)
    - [ ] mail (plugins/mail)

- [x] doc (doc/)

- plugins
  - [x] grpc (plugins/acts-plugin-grpc)
  - [x] web (plugins/acts-plugin-web)
  - [x] nats (plugins/acts-plugin-nats)
  - [ ] obs (plugins/acts-plugin-obs)

