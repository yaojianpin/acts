# Acts workflow engine
`acts` is a fast, tiny, extensiable workflow engine, which provides the abilities to execute workflow based on simple yml model.

The yml workflow model is not as same as the tranditional flow. such as bpmn.  It is inspired by Github actions. As a contrast, it added branch defination for more complex flow, for the purpose of business approval flow, it defines the `subject` property in step to support the top absolute rules for user, org and role. 

**node** new version has changed from `yao` to `acts` since 0.1.1

## Fast
Uses rust to create the lib, there is no virtual machine, no db dependencies. The default store uses rocksdb for local storage.

## Tiny
The lib size is only 3mb (no local db)

## Extensiable
Supports the plugin to extend the functions


## Examples

Here are some examples:

### How to start
First, you should load a ymal workflow model, and call `engine.start` to start and call `engine.close` to stop it.

```no_run
use acts::{ActionOptions, Engine, Vars, State, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start().await;

    let text = include_str!("../examples/simple/model.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    let mut vars = Vars::new();
    vars.insert("input".into(), 3.into());
    workflow.set_env(vars);

    let executor = engine.executor();
    executor.deploy(&workflow).expect("fail to deploy workflow");
    executor.start(&workflow.id, ActionOptions {
            biz_id: Some("w1".to_string()),
            ..Default::default()
        });

    let e = engine.clone();
    engine.emitter().on_complete(move |w: &State<Workflow>| {
        println!("outputs: {:?}", w.outputs());
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

The `subject` is used to create the user [`Act`], which can wait util calling the `post_message` to complete by user.
```yml
name: model name
jobs:
  - id: job1
    steps:
      - name: step1
        subject: 
            matcher: any
            users: |
                let a = ["u1"];
                let b = ["u2"];
                a.union(b)
```
It will generate the user act and send message automationly according to the sub users.
The `matcher` tells the workflow how to pass the step act when you `post_message` to workflow, there are several match rules.

* matcher
1. **one** to generate or check only one user

2. **any** to match any of the users

3. **some(rule_name)** to match some users by giving rule name, which can be registed by `register_some_rule` function.

4. **ord** or **ord(rule_name)** to generate the act message one by one by giving rule name, which can be registed by `register_ord_rule` function

* users
Used to generate the step participants.

The code `role("test_role")` uses the role rule to get the users through the role `test_role`
```yml
users: |
    let users = role("test_role");
    users
```
The following code uses the `user("test_user")` the get the user and then throght the `relate` rule to find the user's owner of the department (`d.owner`).
```yml
users: |
    let users = user("test_user").relate("d.owner");
    users
```

### Use builder to create model
```rust
use acts::{Workflow};

let mut workflow = Workflow::new()
        .with_name("workflow builder")
        .with_output("result", 0.into())
        .with_job(|job| {
            job.with_id("job1")
                .with_env("index", 0.into())
                .with_env("result", 0.into())
                .with_step(|step| {
                    step.with_id("cond")
                        .with_branch(|branch| {
                            branch
                                .with_if(r#"env.get("index") <= env.get("count")"#)
                                .with_step(|step| {
                                    step.with_id("c1")
                                        .with_action(|env| {
                                            let result =
                                                env.get("result").unwrap().as_i64().unwrap();
                                            let index = env.get("index").unwrap().as_i64().unwrap();
                                            env.set("result", (result + index).into());
                                            env.set("index", (index + 1).into());
                                        })
                                        .with_next("cond")
                                })
                        })
                        .with_branch(|branch| {
                            branch.with_if(r#"env.get("index") > env.get("count")"#)
                        })
                })
                .with_step(|step| {
                    step.with_name("step2")
                        .with_action(|env| println!("result={:?}", env.get("result").unwrap()))
                })
        });
```



