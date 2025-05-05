mod plugin;

use acts::{EngineBuilder, Vars, Workflow};

#[tokio::main]
async fn main() {
    let engine = EngineBuilder::new()
        .add_plugin(&plugin::MyPackagePlugin)
        .build()
        .start();

    let (s1, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor();

    let mut vars = Vars::new();
    vars.set("input", 10);

    println!("inputs: {:?}", vars);

    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    engine.executor().model().deploy(&workflow).unwrap();

    executor
        .proc()
        .start(&workflow.id, &vars)
        .expect("start workflow");
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        // println!("on_message: e={:?}", e);

        if e.is_uses("pack3") && e.is_state(acts::MessageState::Created) {
            let params = serde_json::from_value::<plugin::Pack3>(
                e.inputs.get::<serde_json::Value>("params").unwrap(),
            )
            .unwrap();
            println!("func: {}", params.func);
            println!("options.a: {}", params.options.a);
            println!("options.b: {}", params.options.b);

            executor
                .act()
                .complete(
                    &e.pid,
                    &e.tid,
                    &Vars::new().with("input", params.options.a + 10),
                )
                .expect("failed to complete task");
        }
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
