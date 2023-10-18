use acts::{Engine, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() {
    let client = client::Client::new();

    let engine = Engine::new();
    engine.start();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();

    let executor = engine.executor().clone();
    engine.manager().deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.emitter().on_message(move |e| {
        let ret = client.process(&executor, e);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    engine.emitter().on_complete(move |e| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.outputs()
        );
        e.close();
    });
    engine.eloop().await;
}
