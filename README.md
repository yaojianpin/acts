# Acts workflow engine
`acts` is a fast, tiny, extensiable workflow engine, which provides the abilities to execute workflow based on yml model.

The yml workflow model is not as same as the tranditional flow. such as bpmn.  It is inspired by Github actions. As a contrast, it added branch defination for more complex flow, for the purpose of business approval flow, it defines the `subject` property in step to support the top absolute rules for user, org and role. 

**node** new version has changed from `yao` to `acts` since 0.1.1

## Fast
Uses rust to create the lib, there is no virtual machine, no db dependencies. The feature local_store uses the rocksdb to make sure the store performance. 

## Tiny
The lib size is only 3mb (no local_store), you can use Adapter to create external store.

## Extensiable
Supports for extending the plugin
Supports for creating external store

### How to start
First, you should load a ymal workflow model, and call `engine.start` to start and call `engine.close` to stop it.

```no_run
use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();

    let text = include_str!("../examples/simple/model.yml");
    let mut workflow = Workflow::from_yml(text).unwrap();

    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("fail to deploy workflow");

    let mut vars = Vars::new();
    vars.insert("input".into(), 3.into());
    vars.insert("pid".to_string(), "w1".into());
    executor.start(&workflow.id, &vars);

    let e = engine.clone();
    engine.emitter().on_complete(move |e| {
        println!("outputs: {:?}", e.outputs());
    });
    
}
```

### How to create model

Notices the struct of the yaml, there are different type of node, which is constructed by [`Workflow`], [`Job`], [`Branch`] and [`Step`]. Every workflow can have more jobs, every job can have more steps, a step can have more branches and a branch can have `if` property to judge the condition.

The `env` property can be set in each node, in the `run` scripts, you can use `env` moudle to get(`env.get`) or set(`env.set`) the value

The `run` property is the script based on [rhai script](https://github.com/rhaiscript/rhai)

```yml
name: model name
jobs:
  - id: job1
    env:
      value: 0
    steps:
      - name: step 1
        run: |
          print("step 1")

      - name: step 2
        branches:
          - name: branch 1
            if: ${ env.get("value") > 100 }
            run: |
                print("branch 1");

          - name: branch 2
            if: ${ env.get("value") <= 100 }
            steps:
                - name: step 3
                  run: |
                    print("branch 2")
            
```

In the [`Workflow`], you can set the `outputs` to output the env to use.
```yml
name: model name
outputs:
  output_key:
jobs:
  - id: job1
    steps:
      - name: step1
        run: |
          env.set("output_key", "output value");
```

Use act to interact with client
```yml
name: model name
outputs:
  output_key:
jobs:
  - id: job1
    steps:
      - name: step1
        acts:
          - id: init
            name: my act init
            inputs:
              a: 6
            outputs:
              c:

```

Add workflow `actions` to create custom event with client
```yml
name: model name
actions:
  - name: fn1
    id: fn1
    on: 
      - state: created
        nkind: workflow
      - state: completed
        nkind: workflow
  - name: fn2
    id: fn2
    on: 
      - state: completed
        nid: step2

  - name: fn3
    id: fn3
    on: 
      - state: completed
        nid: step3
    inputs:
      a: ${ env.get("value") }
jobs:
  - id: job1
    steps:
      - name: step1
        acts:
          - id: init
            name: my act init
            inputs:
              a: 6
            outputs:
              c:

```

There is a example to use `for` to generate acts, which can wait util calling the action to complete.
```yml
name: model name
jobs:
  - id: job1
    steps:
      - name: step1
        acts:
          - for:
              by: any
              in: |
                let a = ["u1"];
                let b = ["u2"];
                a.union(b)
```
It will generate the user act and send message automationly according to the `in` collection.
The `by` tells the workflow how to pass the act there are several `by` rules.

* by
1. **all** to match all of the acts to complete

2. **any** to match any of the acts to complete

3. **some(rule)** to match some acts by giving rule name. If there is some rule, it can also generate a some act to ask the client to pass or not.

4. **ord** or **ord(rule)** to generate the act one by one. If there is order rule, it can also generate a rule act to sort the collection.

* in
A collection to generate the acts.

The code `act.role("test_role")` uses the role rule to get the users through the role `test_role`
```yml
in: |
    let users = act.role("test_role");
    users
```
The following code uses the `relate` rule to find the user's owner of the department (`d.owner`).
```yml
users: |
    let users = act.relate("user(test_user).d.owner");
    users
```

### Use builder to create model
```rust
use acts::{Workflow};

let mut workflow = Workflow::new()
        .with_name("workflow builder")
        .with_env("count", 10.into())
        .with_output("result", 0.into())
        .with_job(|job| {
            job.with_id("job1")
                .with_input("index", 0.into())
                .with_step(|step| {
                    step.with_id("cond")
                        .with_branch(|branch| {
                            branch
                                .with_if(r#"env.get("index") <= env.get("count")"#)
                                .with_step(|step| {
                                    step.with_id("c1")
                                        .with_run(r#"
                                          let index = env.get("index");
                                          let value = env.get("value");
                                          env.set("value", value + index);
                                          env.set("index", index + 1);
                                        "#)
                                        .with_next("cond")
                                })
                        })
                        .with_branch(|branch| {
                            branch.with_if(r#"env.get("index") > env.get("count")"#)
                        })
                })
                .with_step(|step| {
                    step.with_name("step2")
                        .with_run(r#"println!("result={:?}", env.get("result").unwrap()))"#)
                })
        });
```



