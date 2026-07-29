use acts::{Engine, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start()?;

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine.executor().model().deploy(&workflow, None)?;
    executor.proc().start(&workflow.id, Vars::new())?;

    let ret = executor.evt().start(
        "my-event-model:event-manual",
        &Vars::new().with("result", 0).into(),
    );
    println!("event-manual: {ret:?}");
    let ret = executor.evt().start(
        "my-event-model:event-hook",
        &Vars::new().with("var1", 10).with("var2", "hello").into(),
    );
    println!("event-hook: {ret:?}");
    let ret = executor
        .evt()
        .start("my-event-model:event-chat", &"hello world".into());
    println!("event-chat: {ret:?}");

    Ok(())
}
