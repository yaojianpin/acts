use acts::{data::Package, Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();

    let data = include_str!("./pack1.js");
    let pack = Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        size: data.len() as u32,
        data: data.as_bytes().to_vec(),
        ..Default::default()
    };
    engine
        .executor()
        .pack()
        .publish(&pack)
        .expect("publish pack1");

    let data = include_str!("./pack2.js");
    let pack = Package {
        id: "pack2".to_string(),
        name: "package 2".to_string(),
        size: data.len() as u32,
        data: data.as_bytes().to_vec(),
        ..Default::default()
    };
    engine
        .executor()
        .pack()
        .publish(&pack)
        .expect("publish pack2");

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    engine.executor().model().deploy(&workflow).unwrap();

    executor
        .proc()
        .start(&workflow.id, &vars)
        .expect("start workflow");
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        println!("on_message: e={:?}", e);
    });
    emitter.on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs
        );
        s1.close();
    });
    emitter.on_error(move |e| {
        println!("on_error: state={}", e.state,);
        s2.close();
    });
    sig.recv().await;
}
