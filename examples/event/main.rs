use acts::{Engine, Result, Vars, Workflow};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // The schedule trigger is polled by the engine timer on its own interval,
    // which defaults to the tick. One second keeps the demo short.
    let engine = Engine::builder().tick_interval_secs(1).start().await?;

    let executor = engine.executor(&acts::Principal::unrestricted());
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    print!("{}", workflow.tree_output());
    executor.model().deploy(&workflow, None).await?;

    let ret = executor
        .evt()
        .start(
            "my-event-model:event-manual",
            &Vars::new().with("result", 0).into(),
        )
        .await;
    println!("event-manual: {ret:?}");
    let ret = executor
        .evt()
        .start(
            "my-event-model:event-hook",
            &Vars::new().with("var1", 10).with("var2", "hello").into(),
        )
        .await;
    println!("event-hook: {ret:?}");
    let ret = executor
        .evt()
        .start("my-event-model:event-chat", &"hello world".into())
        .await;
    println!("event-chat: {ret:?}");

    // manual/chat/hook start through `evt:start`; a schedule trigger has no
    // caller, so the engine refuses to start it that way. Deploying armed the
    // row with the next cron fire.
    let event = executor.evt().get("my-event-model:event-schedule").await?;
    println!(
        "event-schedule armed: cron={:?} next_run={}",
        event.schedule, event.next_run
    );
    let ret = executor
        .evt()
        .start("my-event-model:event-schedule", &Vars::new().into())
        .await;
    println!("event-schedule(manual start): {ret:?}");

    // The engine timer fires the due row with the trigger's default params.
    // Watch completions for the marker carried by those params.
    let (s, sig) = engine.signal(Vars::new()).double();
    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            if e.is_type("workflow")
                && e.inputs.get::<String>("source").as_deref() == Some("schedule")
            {
                println!(
                    "event-schedule fired: pid={} inputs={:?} outputs={:?}",
                    e.pid, e.inputs, e.outputs
                );
                s.send(e.outputs.clone());
            }
        }
    });
    match tokio::time::timeout(Duration::from_secs(10), sig.recv()).await {
        Ok(_) => {
            let event = executor.evt().get("my-event-model:event-schedule").await?;
            println!(
                "event-schedule rolled: last_run={} next_run={}",
                event.last_run, event.next_run
            );
        }
        Err(_) => println!("event-schedule did not fire within 10s"),
    }

    engine.close().await;
    Ok(())
}
