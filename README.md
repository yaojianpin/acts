# Acts workflow engine

[![Build](https://github.com/yaojianpin/acts/actions/workflows/rust.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=rust)
[![Test](https://github.com/yaojianpin/acts/actions/workflows/test.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=test)

Acts is a fast, lightweight, extensiable workflow engine that executes workflows defined in YAML format.

Unlike traditional workflow engines (such as BPMN). Acts uses a message-driven architecture to execute and distribute messages. 

Acts uses Step, Branch, Act to build the workflow. Step and Branch are the workflow stucture to run in sequence or to step into different branch by condition. Act is responsible for the action execution.

## Key Features

### Fast

Write in Rust, No virtual machine.

1. bechmark with memory store

```txt,no_run
load/from_yml           time:   [46.130 µs 46.411 µs 46.666 µs]
                        thrpt:  [21.429 Kelem/s 21.547 Kelem/s 21.678 Kelem/s]
deploy/model            time:   [192.09 µs 197.48 µs 204.39 µs]
                        thrpt:  [4.8927 Kelem/s 5.0639 Kelem/s 5.2059 Kelem/s]
start/proc              time:   [684.94 µs 794.32 µs 955.33 µs]
                        thrpt:  [1.0468 Kelem/s 1.2589 Kelem/s 1.4600 Kelem/s]
act/act                 time:   [20.216 µs 21.544 µs 23.957 µs]
                        thrpt:  [41.741 Kelem/s 46.417 Kelem/s 49.467 Kelem/s]

store_mixed/proc_crud/1000
                        time:   [44.543 µs 45.109 µs 46.292 µs]
                        thrpt:  [21.602 Kelem/s 22.169 Kelem/s 22.450 Kelem/s]
```

### Lightweight

The lib size is about 4.6mb now.

### Extensiable

- store collection extension
  support creating external store, please refer to the code under `crates/src/store/postgres`.

- pakcage extension
  support creating custom package, please refer to the code under `example/custom_pakcage`.

## Installation

The easiest way to get the latest version of `acts` is to install it via `cargo`

```bash
cargo add acts
```

## Documents
[`Chinese`](https://yaojianpin.github.io/acts/zh/)
[`English`](https://yaojianpin.github.io/acts/en/)

## Quickstart

1. Create and start the workflow engine by `engine.new()`.
2. Load a yaml model to create a `workflow`.
3. Deploy the model in step 2 by `engine.executor().model()`.
4. Config events by `engine.channel()`.
5. Start the workflow by `engine.executor().proc()`.

```rust,no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new().start().unwrap();

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
        uses: acts.transform.code
        params: |
            return { data: a + 10 };
    "#;
    let workflow = Workflow::from_yml(model).unwrap();

    let executor = engine.executor();
    executor.model().deploy(&workflow, None).expect("fail to deploy workflow");

    let mut vars = Vars::new();

    // set the input value
    vars.set("a", 0);

    // set the pid or auto generate by engine
    vars.set("pid", "w1");

    // start workflow by model id
    executor.proc().start(&workflow.id, vars).expect("fail to start workflow");

    // create channel to receive messages
    let chan = engine.channel();

    chan.on_start(|e| {
        println!("start: {}", e.start_time);
    });

    chan.on_message(|e| {
        println!("message: {:?}", e);
    });

    chan.on_complete(|e| {
        println!("outputs: {:?} end_time: {}", e.outputs, e.end_time);
    });

    chan.on_error(|e| {
        println!("error on proc id: {} model id: {}", e.pid, e.mid);
    });
}
```

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
    uses: acts.transform.code
    params: |
      // get the a variable
      let v = a + 100;
      // do somthing else
```

The vars can also be set by starting the workflow.

```rust,no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
  let engine = Engine::new().start().unwrap();
  let executor = engine.executor();

  let mut vars = Vars::new();
  vars.set("input", 3);
  vars.set("pid", "w2");

  executor.proc().start("m1", vars);
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

For more acts example, please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

The active backend is created externally and passed to
`EngineBuilder::set_store` — when unset, an in-memory store is used:

```rust,ignore
use acts::{Engine, SqliteStore};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .set_store(Arc::new(SqliteStore::open("data/acts.db").unwrap()))
        .build()
        .start()
        .unwrap();
}
```

Backends exported when the matching feature is enabled:

- `MemoryStore` — in-memory store, no persistence (default when unset)
- `SqliteStore` — requires feature `store-sqlite`
- `PostgresStore` — requires feature `store-postgres`
- `RedisStore` — requires feature `store-redis`
- `NatsStore` — requires feature `store-nats`
- `SledStore` — requires feature `store-sled`

Custom stores can be built by implementing `KvStore` and passed to
`set_store` the same way.

## Package

Please see the example `example/pakcage`.

## Acts-Server

Create a acts-server to interact with clients based on grpc.
please see more from [`acts-server`](https://github.com/yaojianpin/acts-server)

## Client channels

- rust https://github.com/yaojianpin/acts-channel
- python https://github.com/yaojianpin/acts-channel-py
- go https://github.com/yaojianpin/acts-channel-go

## Roadmap

acts:

- runtime

  - [x] model (Workflow, Branch, Step, Act)
  - [x] scheduler (Config, Builder, Node, Process, Task, Queue, Event)
  - [x] javascript runner
  - [x] cache
  - [x] plugin register
  - [x] package register
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
  - [x] nats
  - [x] redis
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

  - transform
    - [x] set
    - [x] code

- [x] doc (doc/)
- plugins
  - [x] grpc (plugins/acts-plugin-grpc)
  - [x] web (plugins/acts-plugin-web)
- packages
  - [ ] form (plugins/form)
  - [ ] ai (plugins/ai)
  - [x] state (packages/acts-package-state)
  - [x] http (packages/acts-package-http)
  - [x] shell (packages/acts-package-shell) support nushell, bash and powershell
  - [x] pubsub (packages/acts-package-nats)
  - [ ] observability (plugins/obs)
  - [ ] database (plugins/database)
  - [ ] mail (plugins/mail)
