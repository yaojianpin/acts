use acts::{Engine, State, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let mut workflow = Workflow::from_str(text).unwrap();

    let mut vars = Vars::new();
    vars.insert("input".into(), 100.into());
    workflow.set_env(vars);

    executor.start(&workflow);

    let e = engine.clone();
    engine.emitter().on_complete(move |s: &State<Workflow>| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            s.state,
            s.cost(),
            s.outputs()
        );

        s.print_tree().unwrap();
        e.close();
    });
    engine.start().await;
}
