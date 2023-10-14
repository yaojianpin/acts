use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());
    executor.start(&workflow.id, &vars).expect("start workflow");

    engine.emitter().on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs()
        );
        e.close();
    });
    engine.eloop().await;
}
