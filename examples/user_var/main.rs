mod module;
mod plugin;

use acts::{EngineBuilder, Result, Vars, Workflow};

#[tokio::main]
async fn main() -> Result<()> {
    let engine = EngineBuilder::new()
        .add_plugin(&plugin::UserVarPlugin)
        .build()
        .await?
        .start();

    let model = r#"
    id: my_model
    name: my model
    steps:
      - name: step 1
        acts:
          - uses: acts.core.msg
            inputs:
                test:
                    var1: "changed_var1"
            params:
                var1: '{{ test.var1 }}'
      - name: step 2
        acts:
          - uses: acts.transform.code
            params: |
                let var2 = test.var2;
                console.log("test.var2 = " + var2)
                return { data: var2 }
    "#;
    let workflow = Workflow::from_yml(model).unwrap();

    let (s1, s2) = engine.signal::<()>(()).double();
    let executor = engine.executor();
    executor
        .model()
        .deploy(&workflow)
        .expect("fail to deploy workflow");

    // set test data when start
    let vars = Vars::new().with("a", 0).with("pid", "w1").with(
        "test",
        Vars::new().with("var1", "test_var").with("var2", 100),
    );
    executor
        .proc()
        .start(&workflow.id, &vars)
        .expect("fail to start workflow");
    let chan = engine.channel();

    chan.on_message(|e| {
        if e.is_msg() {
            println!("msg.params: {:?}", e.inputs.get::<Vars>("params"));
        }
    });

    chan.on_complete(move |e| {
        println!("outputs: {:?} cost: {}ms", e.outputs, e.cost());
        s2.close();
    });

    s1.recv().await;

    Ok(())
}
