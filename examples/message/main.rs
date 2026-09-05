use acts::{ChannelOptions, Engine, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start().await?;

    let executor = engine.executor();
    let (s, sig) = engine.signal(()).double();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.executor().model().deploy(&workflow, None).await?;

    executor.proc().start(&workflow.id, Vars::new()).await?;

    // channel messages will store in db
    engine
        .channel_with_options(&ChannelOptions {
            id: "client1".to_string(),
            ..Default::default()
        })
        .on_message(move |message| async move {
            println!(
                "on_message: node id={} type={} state={} inputs={}",
                message.nid, message.r#type, message.state, message.inputs
            );
        });

    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            println!("on_complete: {:?}", e.outputs);
            s.close();
        }
    });
    sig.recv().await;

    Ok(())
}
