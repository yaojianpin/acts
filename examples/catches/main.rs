use std::sync::Arc;

use acts::{Engine, Result, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Arc::new(client::Client::new());

    let engine = Engine::new().start().await?;
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();

    let executor = engine.executor().clone();
    engine.executor().model().deploy(&workflow, None).await?;

    engine.channel().on_message(move |e| {
        let client = client.clone();
        let executor = executor.clone();
        async move {
            println!("on_message: state={} inputs={}", e.state, e.inputs);
            if let Err(err) = client.process(&executor, &e).await {
                eprintln!("{}", err);
                std::process::exit(1);
            }
        }
    });

    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            println!(
                "on_workflow_complete: pid={} cost={}ms outputs={:?}",
                e.pid,
                e.cost(),
                e.outputs
            );
            s.close();
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
