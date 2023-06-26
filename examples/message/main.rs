use acts::{Engine, Message, Vars, Workflow, WorkflowState};

#[tokio::main]
async fn main() {
    let engine = Engine::new();
    engine.start();
    let executor = engine.executor();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_str(text).unwrap();
    engine.manager().deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.emitter().on_message(move |message: &Message| {
        println!("on_message: {:?}", message);
        if let Some(msg) = message.as_user_message() {
            let ret = executor.complete(msg.pid, &msg.aid, &Vars::new());
            if ret.is_err() {
                eprintln!("{}", ret.err().unwrap());
                std::process::exit(1);
            }
        }
    });

    let e = engine.clone();
    engine.emitter().on_complete(move |w: &WorkflowState| {
        println!("{:?}", w.outputs());
        e.close();
    });

    engine.eloop().await;
}
