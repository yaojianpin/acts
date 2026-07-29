use acts::{Engine, Result, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = client::Client::new();

    let engine = Engine::new().start()?;
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();

    let executor = engine.executor().clone();
    engine.executor().model().deploy(&workflow, None)?;
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .expect("start workflow");

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
        s.close();
    });
    sig.recv().await;
    Ok(())
}
