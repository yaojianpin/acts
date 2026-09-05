use acts::{Engine, Result, Variant, Vars, Workflow};
use nanoid::nanoid;
#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::new().start().await?;
    let (s, sig) = engine.signal(()).double();
    let workflow = Workflow::new()
        .with_id("m1")
        .with_ver("0.1.0")
        .with_var("index", 0)
        .with_var("result", 0)
        .with_expose(
            Variant::new()
                .name("result")
                .r#type(acts::VariantTypes::Number)
                .value(r#"${{ result }}"#),
        )
        .with_step(|step| {
            step.with_id("cond")
                .with_branch(|b| {
                    b.with_if(r#"index <= count"#).with_step(|step| {
                        step.with_next("cond").with_uses_code(
                            "acts.transform.code",
                            r#"
                            $set("index", index + 1);
                            $set("result", result + index);
                            "#,
                        )
                    })
                })
                .with_branch(|b| b.with_if(r#"index > count"#))
        })
        .with_step(|step| step.with_name("step2"));

    workflow.print();
    let executor = engine.executor();
    engine.executor().model().deploy(&workflow, None).await?;

    let mut vars = Vars::new();
    vars.insert("pid".to_string(), nanoid!().into());
    vars.insert("count".into(), 100.into());
    executor.proc().start(&workflow.id, vars).await?;

    engine.channel().on_error(move |e| async move {
        println!("error {:?}", e.state);
    });

    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            println!("on_workflow_complete: {:?}, cost={}ms", e.outputs, e.cost());
            s.close();
        }
    });
    sig.recv().await;

    Ok(())
}
