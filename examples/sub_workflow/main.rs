use acts::{Engine, Executor, Vars, Workflow};
mod client;

#[tokio::main]
async fn main() {
    let client = client::Client::new();

    let engine = Engine::new().start();
    let (s1, s2, sig) = engine.signal(()).triple();
    let exec = engine.executor();
    deploy_model(&exec, include_str!("./model/main.yml"));
    deploy_model(&exec, include_str!("./model/sub.yml"));

    let executor = engine.executor().clone();
    executor
        .proc()
        .start("main", &Vars::new())
        .expect("start workflow");

    engine.channel().on_message(move |e| {
        let ret = client.process(&executor, e);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });
    engine.channel().on_start(move |e| {
        println!(
            "on_workflow_start: mid={} pid={} inputs={:?}\n",
            e.model.id, e.pid, e.inputs
        );
    });
    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: mid={} pid={} cost={}ms outputs={:?}\n",
            e.model.id,
            e.pid,
            e.cost(),
            e.outputs
        );

        if e.model.id == "main" {
            s1.close();
        }
    });

    engine.channel().on_error(move |e| {
        eprintln!("on_workflow_error: pid={} state={:?}", e.pid, e.state);
        s2.close();
    });
    sig.recv().await;
}

fn deploy_model(mgr: &Executor, model: &str) {
    let workflow = Workflow::from_yml(model).unwrap();
    mgr.model().deploy(&workflow).expect("deploy model");
}
