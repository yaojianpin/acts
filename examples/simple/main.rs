use yao::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let text = include_str!("./model.yml");
    let mut workflow = Workflow::from_str(text).unwrap();

    let mut vars = Vars::new();
    vars.insert("input".into(), 3.into());
    workflow.set_env(vars);

    engine.push(&workflow);

    let e = engine.clone();
    engine.on_workflow_complete(move |w: &Workflow| {
        println!(
            "on_workflow_complete: {:?}, cost={}ms",
            w.outputs(),
            w.cost()
        );
        e.close();
    });
    engine.start().await;
}
