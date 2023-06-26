use acts::{Engine, Vars, Workflow, WorkflowState};
use nanoid::nanoid;
#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();

    let mut workflow = Workflow::new()
        .with_name("workflow builder")
        .with_id("m1")
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
                                        .with_run(
                                            r#"let index = env.get("index");
                                            let value = env.get("result");
                                            env.set("index", index + 1);
                                            env.set("result", value + index);
                                        "#,
                                        )
                                        .with_next("cond")
                                })
                        })
                        .with_branch(|branch| {
                            branch.with_if(r#"env.get("index") > env.get("count")"#)
                        })
                })
                .with_step(|step| step.with_name("step2"))
        });

    let mut vars = Vars::new();
    vars.insert("count".into(), 100.into());
    workflow.set_env(&vars);

    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("biz_id".to_string(), nanoid!().into());
    executor.start(&workflow.id, &vars).expect("start workflow");

    let e = engine.clone();
    engine.emitter().on_error(|err| {
        println!("error {:?}", err.state);
    });
    engine.emitter().on_complete(move |w: &WorkflowState| {
        println!(
            "on_workflow_complete: {:?}, cost={}ms",
            w.outputs(),
            w.cost()
        );
        e.close();
    });
    engine.eloop().await;
}
