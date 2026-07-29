use acts::{Engine, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start()?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    workflow.print();
    engine.executor().model().deploy(&workflow, None)?;

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());
    executor.proc().start(&workflow.id, vars)?;

    engine.channel().on_error(move |e| {
        print!("on_error: {e:?}");
        s1.close();
    });
    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs
        );
        s2.close();
    });
    sig.recv().await;

    Ok(())
}
