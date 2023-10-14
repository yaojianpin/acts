use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();
    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.manager().deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.emitter().on_message(move |message| {
        println!("on_message: {:?}", message);
    });

    engine.emitter().on_complete(move |e| {
        println!("{:?}", e.outputs());
        e.close();
    });

    engine.eloop().await;
}
