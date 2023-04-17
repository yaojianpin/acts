use acts::{ActionOptions, Engine, Message, State, Workflow};

mod adapter;

#[tokio::main]
async fn main() {
    let mut adapter = adapter::Adapter::new();
    adapter.add_user(
        adapter::User {
            id: "1",
            name: "John",
        },
        "admin",
    );

    adapter.add_user(
        adapter::User {
            id: "2",
            name: "Tom",
        },
        "admin",
    );

    let engine = Engine::new();
    engine
        .adapter()
        .set_role_adapter("example_role", adapter.clone());

    engine.start().await;
    let text = include_str!("./approve.yml");
    let workflow = Workflow::from_str(text).unwrap();

    let executor = engine.executor();
    executor.deploy(&workflow).expect("deploy model");
    executor
        .start(&workflow.id, ActionOptions::default())
        .expect("start workflow");

    engine.emitter().on_message(move |message: &Message| {
        // println!("on_message: {:?}", &message);
        let uid = message.uid.clone().unwrap();
        let ret = executor.next(&message.pid, &uid, None);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.emitter().on_complete(move |w: &State<Workflow>| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            w.pid(),
            w.cost(),
            w.outputs()
        );
        e2.close();
    });

    engine.r#loop().await;
}
