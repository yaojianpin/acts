# Acts workflow engine

[![Build](https://github.com/yaojianpin/acts/actions/workflows/rust.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=rust)
[![Test](https://github.com/yaojianpin/acts/actions/workflows/test.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=test)

`acts` is a fast, tiny, extensiable workflow engine, which provides the abilities to execute workflow based on yml model.

The yml workflow model is not as same as the tranditional workflow. such as `bpmn`. The yml format is inspired by Github actions. The main point of this workflow is to create a top abstraction to run the workflow logic and interact with the client via `act` node.

This workflow engine focus on the workflow logics itself and message distributions. the complex business logic will be completed by `act` via the act message.

## Key Features

### Fast

Uses rust to create the lib, there is no virtual machine, no db dependencies. It also provides the feature `store` to enable the local store.

1. bechmark with memory store

```txt,no_run
load                    time:   [57.334 µs 61.745 µs 66.755 µs]
deploy                  time:   [21.323 µs 23.811 µs 26.829 µs]
start                   time:   [80.320 µs 82.188 µs 84.336 µs]
act                     time:   [601.40 µs 636.69 µs 674.49 µs]
```

### Tiny

The lib size is only 3mb (no store), 4mb(embeded sqlite) you can also use Adapter to create external store.

### Extensiable

Supports for extending the plugin
Supports for creating external store, please refer to the code under `src/store/db/local`.

## Installation

The easiest way to get the latest version of `acts` is to install it via `cargo`

```bash
cargo add acts
```

## Build

If you are using `store` feature, For Windows, recommeded [`MSYS2`](https://www.msys2.org/) and toolchain of stable-x86_64-pc-windows-gnu

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

    let text = include_str!("../examples/simple/model.yml");
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

The model is a yaml format file. where there are different type of node, including [`Workflow`], [`Branch`], [`Step`] and [`Act`]. Every workflow can have more steps, a step can have more branches. In a step, it consists of many acts to complete the step task, such as 'irq', 'msg', 'each', 'chain', 'set', 'expose' and so on. these acts are responsible to act with client or do a single task simplely.

The `run` property is the script based on `javascript`
The `inputs` property can be set the initialzed vars in each node.

```yml
name: model name
inputs:
  value: 0
steps:
  - name: step 1
    run: |
      print("step 1")

  - name: step 2
    branches:
      - name: branch 1
        if: ${ $("value") > 100 }
        run: |
          print("branch 1");

      - name: branch 2
        if: ${ $("value") <= 100 }
        steps:
          - name: step 3
            run: |
              print("branch 2")
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

In `workflow` node, you can setup acts by `setup`.

The act `msg` is to send a message to client.
For more acts, please see the comments as follow:

```yml
name: model name
setup:
setup:
  # set the data by !set
  - act: set
    inputs:
      a: ["u1", "u2"]
      v: 10

  # checks the condition and enters into the 'then' acts
  - act: if
    on: $("v") > 0
    then:
      - act: msg
        key: msg2
  # on step created
  - act: on_created
    then:
      - act: msg
        key: msg3

  # on workflow completed
  - act: on_completed
    then:
      - act: msg
        key: msg4
  # on act created
  - act: on_before_update
    then:
      - act: msg
        key: msg5
  # on act completed
  - act: on_updated
    then:
      - act: msg
        key: msg5

  # on step created or completed
  - act: on_step
    then:
      - act: msg
        key: msg3
  # on error catch
  - act: on_catch
    then:
      - on: err1
        then:
          - act: irq
            key: act3
  # expose the data with special keys
  - act: expose
    inputs:
      out:
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

Use the `setup` to setup some acts when the step is creating.

The acts are 'irq', 'msg', 'set', 'expose', 'chain', 'each' and 'if', it also includes some hooks, such as 'on_created', 'on_completed', 'on_before_update', 'on_updated', 'on_timeout' and 'on_error_catch'.

```yml
name: a setup example
id: setup
steps:
  - name: step 1
    id: step1
    setup:
      # set the data by !set
      - act: set
        inputs:
          a: ['u1', 'u2']
          v: 10
      # send message with key msg1
      - act: msg
        key: msg1
        inputs:
          data: ${ $("a") }

      # chains and runs 'then' one by one by 'in' data
      - act: chain
        in: $("a")
        then:
          - act: irq
            key: act1

      # each the var 'a'
      - act: each
        in: $("a")
        then:
          # the each will generate two "irq" with `act_index`  and `act_value`
          # the `act_index` is the each index. It is 0 and 1 in this example
          # the `act_value` is the each data. It is 'u1' and 'u2' in this example
          - act: irq
            key: act2
      # checks the condition and enters into the 'then' acts
      - act: if
        on: $("v") > 0
        then:
          - act: msg
            key: msg2
      # on step created
      - act: on_created
        then:
          - act: msg
            key: msg3

      # on step completed
      - act: on_completed
        then:
          - act: msg
            key: msg4
      # on act created
      - act: on_before_update
        then:
          - act: msg
            key: msg5
      # on act completed
      - act: on_updated
        then:
          - act: msg
            key: msg5

      # on step created or completed
      - act: on_step
        then:
          - act: msg
            key: msg3
      # on error catch
      - act: on_catch
        - on: err1
          then:
            - act: irq
              key: act3
      # on timeout
      - act: on_timeout
        then:
          - on: 6h
            then:
              - act: irq
                key: act3
      # expose the data with special keys
      - act: expose
        inputs:
          out:
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
      - act: irq
        key: init
  - name: step1
    id: step1
    acts:
      - act: irq
        key: act1
    # catch the step errors
    catches:
      - id: catch1
        on: err1
        then:
          - act: irq
            key: act2
      - id: catch2
        on: err2
        then:
          - act: irq
            key: act3
      - id: catch_others

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
      - act: irq
        key: init
  - name: step1
    id: step1
    acts:
      - act: irq
        key: act1
    # check timeout rules
    timeout:
      # 1d means one day
      # triggers act2 when timeout
      - on: 1d
        then:
          - act: irq
            id: act2
      # 2h means two hours
      # triggers act3 when timeout
      - on: 2h
        then:
          - act: irq
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
      - act: msg
        key: msg1
        inputs:
          a: 1

      # irq is an act to send a request from acts server
      # the client can complete the act and pass data to serever
      - act: irq
        key: init
        name: my act init

        # passes data to the act
        inputs:
          a: 6

        # exposes the data to step
        outputs:
          a:

        # limits the data keys when acting
        rets:
          a:
```

For more acts example, please see [`examples`](https://github.com/yaojianpin/acts/tree/main/examples)

## Store

You can enable the store feature using `store`, which uses [`rusqlite`](https://github.com/rusqlite/rusqlite) to build.

To enable feature `store`

```ignore
[dependencies]
acts = { version = "*", features = ["store"] }
```

For external store:

```rust,no_run
use acts::{Engine, EngineBuilder, data::{Model, Proc, Task, Package, Message, Event}, DbSet, StoreAdapter};
use std::sync::Arc;

#[derive(Clone)]
struct TestStore;

impl StoreAdapter for TestStore {
    fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
        todo!()
    }
    fn procs(&self) -> Arc<dyn DbSet<Item =Proc>> {
        todo!()
    }
    fn tasks(&self) -> Arc<dyn DbSet<Item =Task>> {
        todo!()
    }
    fn packages(&self) -> Arc<dyn DbSet<Item =Package>> {
        todo!()
    }
    fn messages(&self) -> Arc<dyn DbSet<Item =Message>> {
        todo!()
    }

    fn events(&self) -> Arc<dyn DbSet<Item =Event>> {
        todo!()
    }
    fn init(&self) {}
    fn close(&self) {}
}

#[tokio::main]
async fn main() {
   // set custom store
 let store = TestStore;
 let engine = EngineBuilder::new().set_store(&store).build().start();
}
```

## Package

`acts` engine intergrates the [`rquickjs`](https://github.com/delskayn/rquickjs) runtime to execute the package, which can extend the engine abilities.
for more information please see the example [`package`](https://github.com/yaojianpin/acts/tree/main/examples/package)

## Acts-Server

Create a acts-server to interact with clients based on grpc.
please see more from [`acts-server`](https://github.com/yaojianpin/acts-server)

## Client channels

- rust https://github.com/yaojianpin/acts-channel
- python https://github.com/yaojianpin/acts-channel-py
- go https://github.com/yaojianpin/acts-channel-go
