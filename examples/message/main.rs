use yao::{Engine, Message, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_str(text).unwrap();
    engine.push(&workflow);

    let e1 = engine.clone();
    engine.on_message(move |message: &Message| {
        println!("on_message: {}", message);
        let ret = e1.post_message(&message.id, "a", "user", Vars::new());
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.on_workflow_complete(move |w: &Workflow| {
        println!("{:?}", w.outputs());
        e2.close();
    });

    engine.start().await;
}
