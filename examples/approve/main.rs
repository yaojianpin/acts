use acts::{Engine, Message, State, Vars, Workflow};

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

    let text = include_str!("./approve.yml");
    let mut workflow = Workflow::from_str(text).unwrap();
    workflow.set_biz_id("workflow1");

    let executor = engine.executor();
    executor.start(&workflow);

    engine.emitter().on_message(move |message: &Message| {
        println!("engine.on_message: {}", &message.id);
        let uid = message.uid.clone().unwrap();
        let ret = executor.complete("workflow1", &uid, None);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.emitter().on_complete(move |w: &State<Workflow>| {
        println!(
            "on_workflow_complete: biz_id={} {:?}",
            w.node.biz_id(),
            w.outputs()
        );
        e2.close();
    });

    engine.start().await;
}
