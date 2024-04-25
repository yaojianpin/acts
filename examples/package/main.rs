use acts::{data::Package, Engine, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = Engine::new();

    let sig = engine.signal(());
    let s1 = sig.clone();
    let s2 = sig.clone();

    let executor = engine.executor();
    let data = include_str!("./pack1.js");

    let pack = Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        size: data.len() as u32,
        file_data: data.as_bytes().to_vec(),
        ..Default::default()
    };
    engine.manager().publish(&pack).expect("publish package");

    let mut vars = Vars::new();
    vars.insert("input".into(), 10.into());

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    engine.manager().deploy(&workflow).unwrap();

    executor.start(&workflow.id, &vars).expect("start workflow");
    let emitter = engine.emitter();
    emitter.on_message(move |e| {
        println!("on_message: e={:?}", e);
    });
    emitter.on_complete(move |e| {
        println!(
            "on_workflow_complete: state={} cost={}ms output={:?}",
            e.state,
            e.cost(),
            e.outputs()
        );
        s1.close();
    });
    emitter.on_error(move |e| {
        println!("on_error: state={}", e.state,);
        s2.close();
    });
    sig.recv().await;
}
