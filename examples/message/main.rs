use acts::{ChannelOptions, Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let executor = engine.executor();
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.manager().deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    // channel messages will store in db
    engine
        .channel(&ChannelOptions {
            id: "client1".to_string(),
            ..Default::default()
        })
        .on_message(move |message| {
            println!("on_message: {:?}", message);
        });

    engine.emitter().on_complete(move |e| {
        println!("on_complete: {:?}", e.outputs);
        s.close();
    });
    sig.recv().await;
}
