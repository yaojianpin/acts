use acts::{Engine, Result, Vars, Workflow};
use acts_package_javascript::CodePackage;

/// The `acts.transform.code.javascript` package runs JavaScript in an embedded QuickJS
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
        .with_var("value", 21)
        .with_step(|step| {
            step.with_id("double").with_uses_code(
                "acts.transform.code.javascript",
                r#"
                return {
                    doubled: ${{ value }} * 2,
                    upper: "hello".toUpperCase(),
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
            println!("on_workflow_complete: {:?}", e.outputs);
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
