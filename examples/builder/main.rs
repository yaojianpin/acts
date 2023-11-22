use acts::{Engine, Vars, Workflow};
use nanoid::nanoid;
#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();

    let workflow = Workflow::new()
        .with_id("m1")
        .with_env("index", 0.into())
        .with_env("result", 0.into())
        .with_output("result", 0.into())
        .with_step(|step| {
            step.with_id("cond")
                .with_branch(|b| {
                    b.with_if(r#"env.get("index") <= env.get("count")"#)
                        .with_step(|step| {
                            step.with_next("cond").with_run(
                                r#"let index = env.get("index");
                                    let value = env.get("result");
                                    env.set("index", index + 1);
                                    env.set("result", value + index);
                                "#,
                            )
                        })
                })
                .with_branch(|b| b.with_if(r#"env.get("index") > env.get("count")"#))
        })
        .with_step(|step| step.with_name("step2"));

    workflow.print();
    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("pid".to_string(), nanoid!().into());
    vars.insert("count".into(), 100.into());
    executor.start(&workflow.id, &vars).expect("start workflow");

    engine.emitter().on_error(|e| {
        println!("error {:?}", e.state);
    });

    engine.emitter().on_complete(move |e| {
        println!(
            "on_workflow_complete: {:?}, cost={}ms",
            e.outputs(),
            e.cost()
        );
        e.close();
    });
    engine.eloop().await;
}
