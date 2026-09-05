use acts::{Engine, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start().await?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text)?;
    workflow.print();
    engine.executor().model().deploy(&workflow, None).await?;

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());
    executor.proc().start(&workflow.id, vars).await?;

    engine.channel().on_error(move |e| {
        let s1 = s1.clone();
        async move {
            print!("on_error: {e:?}");
            s1.close();
        }
    });
    engine.channel().on_complete(move |e| {
        let s2 = s2.clone();
        async move {
            println!(
                "on_workflow_complete: state={} cost={}ms output={:?}",
                e.state,
                e.cost(),
                e.outputs
            );
            s2.close();
        }
    });
    sig.recv().await;

    Ok(())
}
