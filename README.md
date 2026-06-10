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
load                    time:   [57.334 µs 61.745 µs 66.755 µs]
deploy                  time:   [21.323 µs 23.811 µs 26.829 µs]
start                   time:   [80.320 µs 82.188 µs 84.336 µs]
act                     time:   [601.40 µs 636.69 µs 674.49 µs]
```

### Lightweight

The lib size is about 4.6mb now.

### Extensiable

- store collection extension
  support creating external store, please refer to the code under `store/sqlite`.

- pakcage extension
  support creating custom package, please refer to the code under `example/pakcage`.

## Installation

The easiest way to get the latest version of `acts` is to install it via `cargo`

```bash
cargo add acts
```

## Quickstart

1. Create and start the workflow engine by `engine.new()`.
2. Load a yaml model to create a `workflow`.
3. Deploy the model in step 2 by `engine.executor().model()`.
4. Config events by `engine.channel()`.
5. Start the workflow by `engine.executor().model()`.

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
    executor.model().deploy(&workflow).expect("fail to deploy workflow");

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

outputs:
  - name: data
    title: Output data
    type: object

# the event to start the workflow
on:
  - id: event1
    uses: acts.event.manual
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
              in: '{{ list }}'
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

In the [`Workflow`], you can set the `options.exposes` to filter the outputs.

```yml
name: model name
options:
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
      - id: catch1
        on: err1
        steps:
          - name: catch step 1
            uses: acts.core.irq

      - id: catch2
        on: err2
        steps:
          - name: catch step 2
            uses: acts.core.irq

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

## Store

Current, the supported store features:  store-sqlite, store-postgres, store-sled, store-redis, store-nats

You can add custom store support as follow:

```rust,ignore
use acts::{Engine, Result, KvStore, ScanOptions, data};
use std::sync::Arc;

pub struct MyStore;
impl KvStore for MyStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        Ok(())
    }

    fn scan_prefix(&self, key: &str, options: ScanOptions) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(vec![])
    }

}

#[tokio::main]
async fn main() {
    let engine = acts::Engine::new().start().unwrap();
    let store: Arc<dyn KvStore + 'static> = Arc::new(MyStore);
    engine.extender().register_store(store);
}
```

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

  - event
    - [x] manual
    - [x] hook
    - [x] chat
    - [ ] schedule
    
  - transform
    - [x] set
    - [x] code

- [ ] doc (doc/)

- package extension
  - [ ] form (plugins/form)
  - [ ] ai (plugins/ai)
  - [x] state (plugins/state)
  - [x] http (plugins/http)
  - [x] shell (plugins/shell) support nushell, bash and powershell
  - [ ] pubsub (plugins/pubsub)
  - [ ] observability (plugins/obs)
  - [ ] database (plugins/database)
  - [ ] mail (plugins/mail)
