use acts::{ChannelOptions, Engine, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start()?;

    let executor = engine.executor();
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.executor().model().deploy(&workflow)?;

    executor.proc().start(&workflow.id, Vars::new())?;

    // channel messages will store in db
    engine
        .channel_with_options(&ChannelOptions {
            id: "client1".to_string(),
            ..Default::default()
        })
        .on_message(move |message| {
            if message.is_type("act") {
                println!("on_message: key={} inputs={}", message.key, message.inputs);
            }
        });

    engine.channel().on_complete(move |e| {
        println!("on_complete: {:?}", e.outputs);
        s.close();
    });
    sig.recv().await;

    Ok(())
}
