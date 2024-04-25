use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let s2 = sig.clone();

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());
    executor.start(&workflow.id, &vars).expect("start workflow");
    engine.emitter().on_error(move |e| {
        print!("on_error: {e:?}");
        s1.close();
    });
    engine.emitter().on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs()
        );
        s2.close();
    });
    sig.recv().await;
}
