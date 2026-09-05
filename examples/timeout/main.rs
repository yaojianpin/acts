use std::sync::Arc;

use acts::{Engine, Result, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Arc::new(client::Client::new());
    let engine = Engine::builder()
        .tick_interval_secs(1)
        .build()
        .start()
        .await?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    workflow.print();

    let executor = engine.executor().clone();
    engine.executor().model().deploy(&workflow, None).await?;

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

    engine.channel().on_complete(move |e| {
        let s1 = s1.clone();
        async move {
            println!(
                "on_workflow_complete: pid={} cost={}ms outputs={:?}",
                e.pid,
                e.cost(),
                e.outputs
            );
            s1.close();
        }
    });

    engine.channel().on_error(move |e| {
        let s2 = s2.clone();
        async move {
            println!(
                "on_workflow_error: pid={} cost={}ms state={:?}",
                e.pid,
                e.cost(),
                e.state
            );
            s2.close();
        }
    });
    engine
        .executor()
        .proc()
        .start(&workflow.id, Vars::new())
        .await?;

    sig.recv().await;

    Ok(())
}
