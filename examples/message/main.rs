use acts::{Engine, Message, State, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    let executor = engine.executor();
    let biz_id = "workflow1";

    let text = include_str!("./model.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    workflow.set_biz_id(biz_id);
    executor.start(&workflow);

    engine.emitter().on_message(move |message: &Message| {
        println!("on_message: {:?}", message);
        let ret = executor.complete(biz_id, "user", None);
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

    engine.start().await;
}
