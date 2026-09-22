use acts::{Engine, Result, Vars, Workflow};
use acts_package_javascript::CodePackage;

/// The `acts.app.javascript` package runs JavaScript in an embedded QuickJS
/// runtime. Task data reaches the script through `${{ }}` expressions (CEL,
/// evaluated by the engine), and the script hands data back by returning a
/// JSON object.
#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::builder()
        .add_package::<CodePackage>()
        .start()
        .await?;

    let (s, sig) = engine.signal(()).double();

    let workflow = Workflow::new()
        .with_id("code_demo")
        .with_var("value", 10)
        .with_step(|step| {
            step.with_id("calc").with_uses_code(
                "acts.app.javascript",
                r#"
                let total = 0;
                for(let i = 0; i <= ${{ value }}; i++) {
                    total += i;
                }
                return {
                    title: "total value:".toUpperCase(),
                    total,
                };
                "#,
            )
        })
        .with_step(|step| step.with_id("done"));

    workflow.print();

    let executor = engine.executor(&acts::Principal::unrestricted());
    executor.model().deploy(&workflow, None).await?;

    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            println!("on_workflow_complete: {:?} cost: {}ms", e.outputs, e.cost());
            s.close();
        }
    });

    executor
        .proc()
        .start(&workflow.id, Vars::new().with("pid", "code_demo_1"))
        .await?;

    sig.recv().await;
    Ok(())
}
