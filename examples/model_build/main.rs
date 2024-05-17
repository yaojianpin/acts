use acts::{Engine, Vars, Workflow};
use nanoid::nanoid;
#[tokio::main]
async fn main() {
    let engine = Engine::new();
    let (s, sig) = engine.signal(()).double();
    let workflow = Workflow::new()
        .with_id("m1")
        .with_input("index", 0.into())
        .with_input("result", 0.into())
        .with_output("result", r#"${ $("result") }"#.into())
        .with_step(|step| {
            step.with_id("cond")
                .with_branch(|b| {
                    b.with_if(r#"$("index") <= $("count")"#).with_step(|step| {
                        step.with_next("cond").with_run(
                            r#" let index = $("index");
                                let value = $("result");
                                $("index", index + 1);
                                $("result", value + index);
                                "#,
                        )
                    })
                })
                .with_branch(|b| b.with_if(r#"$("index") > $("count")"#))
        })
        .with_step(|step| step.with_name("step2"));

    workflow.print();
    let executor = engine.executor();
    engine.manager().deploy(&workflow).expect("deploy model");

    let mut vars = Vars::new();
    vars.insert("pid".to_string(), nanoid!().into());
    vars.insert("count".into(), 100.into());
    executor.start(&workflow.id, &vars).expect("start workflow");

    engine.channel().on_error(|e| {
        println!("error {:?}", e.state);
    });

    engine.channel().on_complete(move |e| {
        println!("on_workflow_complete: {:?}, cost={}ms", e.outputs, e.cost());
        s.close();
    });
    sig.recv().await;
}
