use act::{Engine, Message, Vars, Workflow};

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
    engine.push(&workflow);

    let e1 = engine.clone();
    engine.on_message(move |message: &Message| {
        println!("engine.on_message: {}", &message.id);
        let vars = Vars::new();
        let ret = e1.post_message(&message.id, "a", &message.user, vars);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }
    });

    let e2 = engine.clone();
    engine.on_workflow_complete(move |w: &Workflow| {
        w.tree();
        e2.close();
    });

    engine.start().await;
}
