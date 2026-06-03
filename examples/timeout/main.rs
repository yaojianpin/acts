use acts::{EngineBuilder, Result, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = client::Client::new();
    let engine = EngineBuilder::new().tick_interval_secs(1).build().start()?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    workflow.print();

    let executor = engine.executor().clone();
    engine.executor().model().deploy(&workflow)?;

    engine.channel().on_message(move |e| {
        let ret = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(client.process(&executor, e))
        });
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.outputs
        );
        s1.close();
    });

    engine.channel().on_error(move |e| {
        println!(
            "on_workflow_error: pid={} cost={}ms state={:?}",
            e.pid,
            e.cost(),
            e.state
        );
        s2.close();
    });
    engine.executor().proc().start(&workflow.id, Vars::new())?;

    sig.recv().await;

    Ok(())
}
