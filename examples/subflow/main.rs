use std::sync::Arc;

use acts::{Engine, Executor, Result, Vars, Workflow};
mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Arc::new(client::Client::new());

    let engine = Engine::new().start().await?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let exec = engine.executor();
    deploy_model(&exec, include_str!("./model/main.yml")).await?;
    deploy_model(&exec, include_str!("./model/sub.yml")).await?;

    let executor = engine.executor().clone();
    engine.channel().on_message(move |e| {
        let client = client.clone();
        let executor = executor.clone();
        async move {
            if let Err(err) = client.process(&executor, &e).await {
                eprintln!("{}", err);
                std::process::exit(1);
            }
        }
    });
    engine.channel().on_start(move |e| async move {
        println!(
            "on_workflow_start: mid={} pid={} inputs={:?}\n",
            e.mid, e.pid, e.inputs
        );
    });
    engine.channel().on_complete(move |e| {
        let s1 = s1.clone();
        async move {
            println!(
                "on_workflow_complete: mid={} pid={} cost={}ms outputs={:?}\n",
                e.mid,
                e.pid,
                e.cost(),
                e.outputs
            );

            if e.mid == "main" {
                s1.close();
            }
        }
    });

    engine.channel().on_error(move |e| {
        let s2 = s2.clone();
        async move {
            eprintln!(
                "on_workflow_error: pid={} state={:?} data={:?}",
                e.pid, e.state, e
            );
            s2.close();
        }
    });

    engine.executor().proc().start("main", Vars::new()).await?;

    sig.recv().await;

    Ok(())
}

async fn deploy_model(mgr: &Executor, model: &str) -> Result<()> {
    let workflow = Workflow::from_yml(model)?;
    mgr.model().deploy(&workflow, None).await?;
    Ok(())
}
