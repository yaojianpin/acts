use acts::{Engine, Vars, Workflow, WorkflowState};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_str(text).unwrap();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());
    executor.start(&workflow.id, &vars).expect("start workflow");

    let e = engine.clone();
    engine.emitter().on_complete(move |s: &WorkflowState| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            s.state,
            s.cost(),
            s.outputs()
        );

        //s.tree().unwrap();
        e.close();
    });
    engine.eloop().await;
}
