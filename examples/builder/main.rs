use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

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

    let mut vars = Vars::new();
    vars.insert("count".into(), 100.into());
    workflow.set_env(vars);

    engine.push(&workflow);

    let e = engine.clone();
    engine.on_workflow_complete(move |w: &Workflow| {
        println!(
            "on_workflow_complete: {:?}, cost={}ms",
            w.outputs(),
            w.cost()
        );
        w.tree();
        e.close();
    });
    engine.start().await;
}
