# Acts workflow engine
[![Build](https://github.com/yaojianpin/acts/actions/workflows/rust.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=rust)

[![Test](https://github.com/yaojianpin/acts/actions/workflows/test.yml/badge.svg)](https://github.com/yaojianpin/acts/actions?workflow=test)

`acts` is a fast, tiny, extensiable workflow engine, which provides the abilities to execute workflow based on yml model.

The yml workflow model is not as same as the tranditional workflow. such as `bpmn`.  The yml format is inspired by Github actions.  The main point of this workflow is to create a top abstraction to run the workflow logic and interact with the client via `act` node.

This workflow engine focus on the workflow logics itself and message distributions. the complex business logic will be completed by `act` via the act message. 

## Key Features

### Fast
Uses rust to create the lib, there is no virtual machine, no db dependencies. It also provides the feature `store` to enable the local store. 

1. bechmark with memory store
```txt,no_run
load                    time:   [66.438 µs 75.248 µs 84.207 µs]
deploy                  time:   [6.612 µs 17.356 µs 18.282 µs]
start                   time:   [69.952 µs 70.628 µs 71.287 µs]
act                     time:   [7.9698 ms 8.5588 ms 9.0608 ms]
```

### Tiny
The lib size is only 3mb (no store), you can use Adapter to create external store.

### Extensiable
Supports for extending the plugin
Supports for creating external store, please refer to the code under `src/store/db/local`.

## Installation
The easiest way to get the latest version of `acts` is to install it via `cargo`
```bash
cargo add acts
```

## Quickstart
1. Create and start the workflow engine by `engine.new()`.
2. Load a yaml model to create a `workflow`. 
3. Deploy the model in step 2 by `engine.manager()`.
4. Config events by `engine.emitter()`.
5. Start the workflow by `engine.executor()`.

```rust,no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let text = include_str!("../examples/simple/model.yml");
    let workflow = Workflow::from_yml(text).unwrap();

    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("fail to deploy workflow");

    let mut vars = Vars::new();
    vars.insert("input".into(), 3.into());
    vars.insert("pid".to_string(), "w1".into());
    executor.start(&workflow.id, &vars).expect("fail to start workflow");;
    let emitter = engine.emitter();

    emitter.on_start(|e| {
        println!("start: {}", e.start_time);
    });

    emitter.on_message(|e| {
        println!("message: {:?}", e);
    });

    emitter.on_complete(|e| {
        println!("outputs: {:?} end_time: {}", e.outputs, e.end_time);
    });

    emitter.on_error(|e| {
        println!("error on proc id: {} model id: {}", e.pid, e.model.id);
    });
}
```

## Examples

Please see [`examples`](<https://github.com/yaojianpin/acts/tree/main/examples>)

## Model Usage

The model is a yaml format file. where there are different type of node, including [`Workflow`], [`Branch`], [`Step`] and [`Act`]. Every workflow can have more steps, a step can have more branches. In a step,  it consists of many acts to complete the step task, such as 'req', 'msg', 'each', 'chain', 'set', 'expose' and so on. these acts are responsible to act with client or do a single task simplely.

The `run` property is the script based on [rhai script](https://github.com/rhaiscript/rhai)
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
      env.set("output_key", "output value");
```

The inputs can also be set by starting the workflow.

```rust,no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
  let engine = Engine::new();
  let executor = engine.executor();

  let mut vars = Vars::new();
  vars.insert("input".into(), 3.into());
  vars.insert("pid".to_string(), "w2".into());

  executor.start("m1", &vars);
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
      env.set("output_key", "output value");
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
  - !set
    a: ["u1", "u2"]
    v: 10

  # checks the condition and enters into the 'then' acts
  - !if
    on: $("v") > 0
    then:
      - !msg
        id: msg2
  # on step created
  - !on_created
    - !msg
      id: msg3

  # on workflow completed
  - !on_completed
    - !msg
      id: msg4
  # on act created
  - !on_before_update
    - !msg
      id: msg5
  # on act completed
  - !on_updated
    - !msg
      id: msg5

  # on step created or completed
  - !on_step
      - !msg
        id: msg3
  # on error catch
  - !on_error_catch
    - err: err1
      then:
        - !req
          id: act3
  # expose the data with special keys
  - !expose
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

The acts are 'req', 'msg', 'set', 'expose', 'chain', 'each' and 'if',  it also includes some hooks, such as 'on_created', 'on_completed', 'on_before_update', 'on_updated', 'on_timeout' and 'on_error_catch'.

```yml
name: a setup example
id: setup
steps:
  - name: step 1
    id: step1
    setup:
  
      # set the data by !set
      - !set
        a: ["u1", "u2"]
        v: 10
      # send message with key msg1
      - !msg
        id: msg1
        inputs:
          data: ${ $("a") }

      # chains and runs 'run' one by one by 'in' data
      - !chain
        in: $("a")
        run:
          - !req
            id: act1

      # each the var 'a'
      - !each
        in: $("a")
        run:
          # the each will generate two !req with `act_index`  and `act_value`
          # the `act_index` is the each index. It is 0 and 1 in this example
          # the `act_value` is the each data. It is 'u1' and 'u2' in this example
          - !req
            id: act2
      # checks the condition and enters into the 'then' acts
      - !if
        on: $("v") > 0
        then:
          - !msg
            id: msg2
      # on step created
      - !on_created
        - !msg
          id: msg3

      # on step completed
      - !on_completed
        - !msg
          id: msg4
      # on act created
      - !on_before_update
        - !msg
          id: msg5
      # on act completed
      - !on_updated
        - !msg
          id: msg5

      # on step created or completed
      - !on_step
          - !msg
            id: msg3
      # on error catch
      - !on_error_catch
        - err: err1
          then:
            - !req
              id: act3
      # on timeout 
      - !on_timeout
        - on: 6h
          then:
            - !req
              id: act3
      # expose the data with special keys
      - !expose
         out:
  - name: final
    id: final
```

For more acts example, please see [`examples`](<https://github.com/yaojianpin/acts/tree/main/examples>)

#### step.catches
Use the `catches` to capture the `step` error.
```yml
name: a catches example
id: catches
steps:
  - name: prepare
    id: prepare
    acts:
      - !req
        id: init
  - name: step1
    id: step1
    acts:
      - !req
        id: act1
    # catch the step errors
    catches:
      - id: catch1
        err: err1
        then:
          - !req
            id: act2
      - id: catch2
        err: err2
        then:
          - !req
            id: act3
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
      - !req
        id: init
  - name: step1
    id: step1
    acts:
      - !req
        id: act1
    # check timeout rules
    timeout:
      # 1d means one day
      # triggers act2 when timeout
      - on: 1d
        then:
          - !req
            id: act2
      # 2h means two hours
      # triggers act3 when timeout
      - on: 2h
        then:
          - !req
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
      - !msg
        id: msg1
        inputs:
          a: 1
          
      # req is a act to send a request from acts server
      # the client can complete the act and pass data to serever
      - !req
        id: init
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

For more acts example, please see [`examples`](<https://github.com/yaojianpin/acts/tree/main/examples>)

## Store
You can enable the store feature using `store`, which uses [`duckdb`](<https://github.com/duckdb/duckdb>) to build.

To enable feature `store`
```ignore
[dependencies]
acts = { version = "*", features = ["store"] }
```

For external store:

 ```rust,no_run
 use acts::{Engine, Builder, data::{Model, Proc, Task, Package, Message}, DbSet, StoreAdapter};
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
     fn init(&self) {}
     fn close(&self) {}
 }

#[tokio::main]
async fn main() {
    // set custom store
  let store = TestStore;
  let engine = Builder::new().store(&store).build();
}
 ```

## Wit package
`acts` engine intergrates the [`ruickjs`](<https://github.com/delskayn/rquickjs>) runtime to execute the package, which can extend the engine abilities.
for more information please see the example [`package`](<https://github.com/yaojianpin/acts/tree/main/examples/package>)

## Acts-Server
Create a acts-server to interact with clients based on grpc.
please see more from [`acts-server`](<https://github.com/yaojianpin/acts-server>)

## Acts-Channel
The channel is used to interact with the server. the actions includes 'deploy', 'start', 'push', 'remove', 'complete', 'back', 'cancel', 'skip', 'abort' and 'error'.

please see more from [`acts-channel`](<https://github.com/yaojianpin/acts-channel>)




