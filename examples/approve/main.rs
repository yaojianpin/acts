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

    engine.start();
    let text = include_str!("./approve.yml");
    let workflow = Workflow::from_str(text).unwrap();
    let executor = engine.executor();
    executor.deploy(&workflow).expect("deploy model");
    executor
        .start(
            &workflow.id,
            ActionOptions {
                biz_id: Some("w1".into()),
                ..Default::default()
            },
        )
        .expect("start workflow");

    engine.emitter().on_message(move |message: &Message| {
        println!("engine.on_message: {}", &message.id);
        let uid = message.uid.clone().unwrap();
        let ret = executor.next("workflow1", &uid, None);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.emitter().on_complete(move |w: &State<Workflow>| {
        println!("on_workflow_complete: biz_id={} {:?}", w.pid(), w.outputs());
        e2.close();
    });

    engine.r#loop().await;
}
