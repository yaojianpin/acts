use acts::{Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new().start();

    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    engine
        .executor()
        .model()
        .deploy(&workflow)
        .expect("deploy model");
    executor
        .proc()
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

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
}
