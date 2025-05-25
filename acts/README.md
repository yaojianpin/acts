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
    let engine = Engine::new().start();

    let text = include_str!("../../examples/simple/model.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    let executor = engine.executor();
    executor.model().deploy(&workflow).expect("fail to deploy workflow");

    let mut vars = Vars::new();
    vars.insert("input".into(), 3.into());
    vars.insert("pid".to_string(), "w1".into());
    executor.proc().start(&workflow.id, &vars).expect("fail to start workflow");;
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
        println!("error on proc id: {} model id: {}", e.pid, e.model.id);
    });
}
```

## Examples

Please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

## Model Usage

The model is a yaml format file. where there are different type of node, including [`Workflow`], [`Branch`], [`Step`] and [`Act`]. 


```yml
name: model name
# workflow.inputs are the global vars
inputs:
  value: 0
# the event to start the workflow
on:
  - id: event1
    uses: acts.event.manual
# workflow steps
steps:
  - name: step 1
    # execute by act
    acts:
        # init with interrupt request to client
        # and make sure complete the action with 'list' var
      - name: init
        uses: acts.core.irq
        outputs:
          list:

  - name: step 2
    # workflow branches to run by condition
    branches:
      - name: branch 1
        if: ${ $("value") > 100 }
        steps:
          - name: step 3
            acts:
              - name: send a message
                uses: acts.core.msg

      - name: branch 2
        if: ${ $("value") <= 100 }
        steps:
          - name: step 4
            acts:
              - name: parallel send irq request
                uses: acts.core.parallel
                params:
                  in: ${ ${list} }
                  acts:
                    - uses: acts.core.irq
  - name: final step

```

### Inputs

In the [`Workflow`], you can set the `inputs` to init the workflow vars.

```yml
name: model name
inputs:
  a: 100
steps:
  - name: step1
    run: |
      $("output_key", "output value");
```

The inputs can also be set by starting the workflow.

```rust,no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
  let engine = Engine::new().start();
  let executor = engine.executor();

  let mut vars = Vars::new();
  vars.insert("input".into(), 3.into());
  vars.insert("pid".to_string(), "w2".into());

  executor.proc().start("m1", &vars);
}
```

### Outputs

In the [`Workflow`], you can set the `outputs` to output the env to use.

```yml
name: model name
outputs:
  output_key:
steps:
  - name: step1
    run: |
      $("output_key", "output value");
```

### Setup

In `workflow` node, you can setup act event by `setup`.

The act `msg` is to send a message to client.
For more acts, please see the comments as follow:

```yml
name: model name
steps:
  - uses: acts.core.set
    params:
      a: ['u1', 'u2']
      v: 10
  - uses: acts.core.msg
    if: $("v") > 0
    key: msg1
setup:
  # on step created
  - uses: acts.core.msg
    on: created
    key: msg3

  # on workflow completed
  - uses: acts.core.msg
    on: completed
    key: msg4

  # on act created
  - uses: acts.core.msg
    on: before_update
    key: msg5

  # on act completed
  - uses: acts.core.msg
    on: updated
    key: msg5

  # on step created or completed
  - uses: acts.core.msg
    on: step
    key: msg3
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

#### step.setup

Use the `setup` to setup some act event for step.

The act event includes 'created', 'completed', 'step', 'before_update' and 'updated'.

```yml
name: a setup example
id: setup
steps:
  - name: step 1
    id: step1
    setup:
      # on step created
      - uses: acts.core.msg
        on: created
        key: msg3

      # on step completed
      - uses: acts.core.msg
        on: completed
        key: msg4

      # on act created
      - uses: acts.core.msg
        on: before_update
        key: msg5

      # on act completed
      - uses: acts.core.msg
        on: updated
        key: msg5

      # on step created or completed
      - uses: acts.core.msg
        on: step
        key: msg3

  - name: final
    id: final
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
    acts:
      - uses: acts.core.irq
        key: init
  - name: step1
    id: step1
    acts:
      - uses: acts.core.irq
        key: act1
    # catch the step errors
    catches:
      - id: catch1
        on: err1
        steps:
          - name: catch step 1
            acts:
              - uses: acts.core.irq
                key: act2

      - id: catch2
        on: err2
        steps:
          - name: catch step 2
            acts:
              - uses: acts.core.irq
                key: act3

  - name: final
    id: final
```

#### step.timeout

Use the `timeout` to check the task time.

```yml
name: a timeout example
id: timeout
steps:
  - name: prepare
    id: prepare
    acts:
      - uses: acts.core.irq
        key: init
  - name: step1
    id: step1
    acts:
      - uses: acts.core.irq
        key: act1
    # check timeout rules
    timeout:
      # 1d means one day
      # triggers act2 when timeout
      - on: 1d
        steps:
          - name: timeout step 1
            acts:
              - uses: acts.core.irq
                id: act2

      # 2h means two hours
      # triggers act3 when timeout
      - on: 2h
        steps:
          - name: timeout step 2
            acts:
              - uses: acts.core.irq
                id: act3

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
        if: $("v") > 0
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

### Acts

Use `acts` to create act to interact with client， or finish a special function through several act type.

```yml
name: model name
outputs:
  output_key:
steps:
  - name: step1
    acts:
      # send message to client
      - uses: acts.core.msg
        key: msg1
        params:
          a: 1

      # irq is an act to send a request from acts server
      # the client can complete the act and pass data to serever
      - uses: acts.core.irq
        key: init
        name: my act init

        # passes data to the act
        params:
          a: 6

        # limits the data keys when acting
        outputs:
          a:
```

For more acts example, please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

## Store

You can add more store support by store plugins. The avaliable store plugins are as follow:
- acts-sqlite

```rust,ignore
use acts::EngineBuilder;
use acts_store_sqlite::SqliteStore;

#[tokio::main]
async fn main() {
  let engine = EngineBuilder::new().add_plugin(&SqliteStore).build().await.unwrap().start();
}
```

- acts-postgres

```rust,ignore
use acts::EngineBuilder;
use acts_store_postgres::PostgresStore;

#[tokio::main]
async fn main() {
  let engine = EngineBuilder::new().add_plugin(&PostgresStore).build().await.unwrap().start();
}
```

How to create custom store plugin, please see the code under `store/`

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

- packages

  - core
    - [x] irq
    - [x] msg
    - [x] block
    - [x] action
    - [x] parallel
    - [x] sequence
    - [x] subflow
    - [ ] http

  - event
    - [x] manual
    - [x] hook
    - [x] chat
    - [ ] schedule
    
  - transform
    - [x] set
    - [x] code
    - [ ] split

- [ ] doc (doc/)

- store extension

  - [x] sqlite
  - [x] postgres

- package extension
  - [ ] form (plugins/form)
  - [ ] ai (plugins/ai)
  - [x] state (plugins/state)
  - [ ] pubsub (plugins/pubsub)
  - [ ] observability (plugins/obs)
  - [ ] database (plugins/database)
  - [ ] mail (plugins/mail)
