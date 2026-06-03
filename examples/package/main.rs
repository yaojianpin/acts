mod plugin;

use acts::{EngineBuilder, Vars, Workflow};

#[tokio::main]
async fn main() -> acts::Result<()> {
    let engine = EngineBuilder::new()
        .add_plugin(&plugin::MyPackagePlugin)
        .build()
        .start()?;

    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();

    let mut vars = Vars::new();
    vars.set("input", 10);

    println!("inputs: {vars:?}");

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    engine.executor().model().deploy(&workflow)?;

    executor.proc().start(&workflow.id, vars)?;
    let emitter = engine.channel();

    emitter.on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs
        );
        s1.close();
    });
    emitter.on_error(move |e| {
        println!("on_error: state={}", e.state,);
        s2.close();
    });
    sig.recv().await;

    Ok(())
}
