use acts::{Engine, Result, Vars, Workflow};
use acts_plugin_web::WebPlugin;

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::builder()
        .add_plugin(&WebPlugin::new())
        .build()
        .start()?;

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    let (s, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor().clone();
    engine
        .executor()
        .model()
        .deploy(&workflow, None)
        .expect("deploy model");
    executor
        .proc()
        .start(&workflow.id, Vars::new())
        .expect("start workflow");

    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.outputs
        );
        s.close();
    });
    engine.channel().on_error(move |e| {
        println!(
            "on_workflow_error: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.inputs
        );
        s2.close();
    });

    println!("Web server running on port 10082...");
    sig.recv().await;

    Ok(())
}
