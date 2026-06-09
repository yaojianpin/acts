mod pack1;
mod pack2;

use acts::{ActPackage, ActPlugin, ChannelOptions, Engine, MessageState, Result};

#[derive(Clone)]
pub struct MyPackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for MyPackagePlugin {
    fn on_init(&self, engine: &Engine) -> Result<()> {
        println!("MyPackagePlugin init");
        engine.extender().register_package(&pack1::Pack1::meta())?;
        engine.extender().register_package(&pack2::Pack2::meta())?;

        let executor = engine.executor();
        engine
            .channel_with_options(&ChannelOptions {
                id: "chan1".to_string(),
                ack: true,
                r#type: "act".to_string(),
                state: "{created,completed}".to_string(),
                uses: "{pack1,pack2,pack3}".to_string(),
                ..Default::default()
            })
            .on_message(move |e| {
                let params: serde_json::Value = e.inputs.get("params").unwrap();
                if e.uses == "pack1" && e.r#type == "act" && e.state == MessageState::Created {
                    let pack1: pack1::Pack1 = serde_json::from_value(params.clone()).unwrap();
                    let ret = pack1.execute();
                    let ex = executor.clone();
                    match ret {
                        Ok(vars) => {
                            ex.act().complete(&e.pid, &e.tid, vars).unwrap();
                        }
                        Err(err) => {
                            ex.act().fail(&e.pid, &e.tid, err.into()).unwrap();
                        }
                    }
                }

                if e.uses == "pack2" && e.r#type == "act" {
                    let pack1: pack2::Pack2 = serde_json::from_value(params.clone()).unwrap();
                    let ret = pack1.execute().unwrap();
                    println!("pack2 result: {ret:?}");
                }
            });

        Ok(())
    }
}
