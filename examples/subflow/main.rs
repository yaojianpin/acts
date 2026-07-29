use acts::{Engine, Executor, Result, Vars, Workflow};
mod client;

#[tokio::main]
async fn main() -> Result<()> {
    let client = client::Client::new();

    let engine = Engine::new().start()?;
    let (s1, s2, sig) = engine.signal(()).triple();
    let exec = engine.executor();
    deploy_model(&exec, include_str!("./model/main.yml"))?;
    deploy_model(&exec, include_str!("./model/sub.yml"))?;

    let executor = engine.executor().clone();
    engine.channel().on_message(move |e| {
        let ret = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(client.process(&executor, e))
        });
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });
    engine.channel().on_start(move |e| {
        println!(
            "on_workflow_start: mid={} pid={} inputs={:?}\n",
            e.mid, e.pid, e.inputs
        );
    });
    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: mid={} pid={} cost={}ms outputs={:?}\n",
            e.mid,
            e.pid,
            e.cost(),
            e.outputs
        );

        if e.mid == "main" {
            s1.close();
        }
    });

    engine.channel().on_error(move |e| {
        eprintln!("on_workflow_error: pid={} state={:?}", e.pid, e.state);
        s2.close();
    });

    engine.executor().proc().start("main", Vars::new())?;

    sig.recv().await;

    Ok(())
}

fn deploy_model(mgr: &Executor, model: &str) -> Result<()> {
    let workflow = Workflow::from_yml(model)?;
    mgr.model().deploy(&workflow, None)?;
    Ok(())
}
