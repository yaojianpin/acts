use acts::{Engine, Vars, Workflow};

mod client;

#[tokio::main]
async fn main() {
    let client = client::Client::new();

    let engine = Engine::new().start();
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();

    let executor = engine.executor().clone();
    engine
        .executor()
        .model()
        .deploy(&workflow)
        .expect("deploy model");

    engine.channel().on_message(move |e| {
        println!("on_message: key={} inputs={}", e.key, e.inputs);
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
        s.close();
    });
    engine
        .executor()
        .proc()
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");
    sig.recv().await;
}
