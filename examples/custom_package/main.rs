mod pack1;
mod pack2;

use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() -> acts::Result<()> {
    let engine = Engine::builder()
        .add_package::<pack1::Pack1>()
        .add_package::<pack2::Pack2>()
        .build()
        .start()?;

    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();

    let mut vars = Vars::new();
    vars.set("input", 10);

    println!("inputs: {vars:?}");

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    workflow.print();
    engine.executor().model().deploy(&workflow, None)?;

    executor.proc().start(&workflow.id, vars)?;
    let chan = engine.channel();

    chan.on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs
        );
        s1.close();
    });
    chan.on_error(move |e| {
        println!("on_error: state={:?}", e);
        s2.close();
    });
    sig.recv().await;

    Ok(())
}
