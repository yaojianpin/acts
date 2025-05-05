use acts::{EngineBuilder, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() {
    let client = client::Client::new();
    let engine = EngineBuilder::new().tick_interval_secs(1).build().start();
    let (s1, s2, sig) = engine.signal(()).triple();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();

    let executor = engine.executor().clone();
    engine
        .executor()
        .model()
        .deploy(&workflow)
        .expect("deploy model");
    executor
        .proc()
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.channel().on_message(move |e| {
        let ret = client.process(&executor, e);
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
    sig.recv().await;
}
