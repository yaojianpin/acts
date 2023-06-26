use acts::{Engine, Message, Vars, Workflow, WorkflowState};

mod client;

#[tokio::main]
async fn main() {
    let mut store = client::Client::new();
    store.init();

    let engine = Engine::new();
    engine.start();
    let text = include_str!("./approve.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.emitter().on_message(move |message: &Message| {
        let ret = store.process(&executor, message);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.emitter().on_complete(move |w: &WorkflowState| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            w.pid,
            w.cost(),
            w.outputs()
        );

        e2.close();
    });

    engine.eloop().await;
}
