use acts::{ActionOptions, Engine, Message, State, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();
    let executor = engine.executor();
    let biz_id = "workflow1";

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_str(text).unwrap();
    executor.deploy(&workflow).expect("deploy model");
    executor
        .start(
            &workflow.id,
            ActionOptions {
                biz_id: Some(biz_id.into()),
                ..Default::default()
            },
        )
        .expect("start workflow");

    engine.emitter().on_message(move |message: &Message| {
        println!("on_message: {:?}", message);
        let ret = executor.next(biz_id, "user", None);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e = engine.clone();
    engine.emitter().on_complete(move |w: &State<Workflow>| {
        println!("{:?}", w.outputs());
        e.close();
    });

    engine.r#loop().await;
}
