mod pack1;
mod pack2;

use acts::{ActPackage, ActPlugin, ChannelOptions, Engine, Result};

#[derive(Clone)]
pub struct MyPackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for MyPackagePlugin {
    async fn on_init(&self, engine: &Engine) -> Result<()> {
        engine
            .extender()
            .register_package(&pack1::Pack1::meta())
            .expect("failed to register Pack1");
        engine
            .extender()
            .register_package(&pack2::Pack2::meta())
            .expect("failed to register Pack2");

        let executor = engine.executor();
        engine
            .channel_with_options(&ChannelOptions {
                id: "chan1".to_string(),
                ack: true,
                r#type: "act".to_string(),
                state: "created".to_string(),
                uses: "{pack1,pack2,pack3}".to_string(),
                ..Default::default()
            })
            .on_message(move |e| {
                let params: serde_json::Value = e.inputs.get("params").unwrap();
                if e.uses == "pack1" {
                    let pack1: pack1::Pack1 = serde_json::from_value(params.clone()).unwrap();
                    let ret = pack1.execute();
                    match ret {
                        Ok(vars) => {
                            executor.act().complete(&e.pid, &e.tid, &vars).unwrap();
                        }
                        Err(err) => {
                            executor.act().error(&e.pid, &e.tid, &err.into()).unwrap();
                        }
                    }
                }

                if e.uses == "pack2" {
                    let pack1: pack2::Pack2 = serde_json::from_value(params.clone()).unwrap();
                    let ret = pack1.execute();
                    println!("pack2 result: {:?}", ret);
                }
            });

        Ok(())
    }
}
