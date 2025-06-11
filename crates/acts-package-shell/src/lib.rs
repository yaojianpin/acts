mod package;

use acts::{ActPackage, ActPlugin, ChannelOptions, Result};

#[derive(Clone)]
pub struct ShellPackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for ShellPackagePlugin {
    async fn on_init(&self, engine: &acts::Engine) -> Result<()> {
        let meta = package::ShellPackage::meta();
        engine.extender().register_package(&meta)?;

        let executor = engine.executor();
        let chan = engine.channel_with_options(&ChannelOptions {
            id: meta.name.to_string(),
            ack: true,
            r#type: "act".to_string(),
            state: "created".to_string(),
            uses: meta.name.to_string(),
            ..Default::default()
        });
        chan.on_message(move |e| {
            let inputs = e.inputs.clone();
            let pid = e.pid.clone();
            let tid = e.tid.clone();
            let executor = executor.clone();
            tokio::spawn(async move {
                let pack = package::ShellPackage::create(&inputs);
                if let Err(err) = pack {
                    executor.act().error(&pid, &tid, &err.into()).unwrap();
                    return;
                }

                let pack = pack.unwrap();
                match pack.run().await {
                    Ok(data) => {
                        executor.act().complete(&pid, &tid, &data).unwrap();
                    }
                    Err(err) => {
                        executor.act().error(&pid, &tid, &err.into()).unwrap();
                    }
                };
            });
        });

        Ok(())
    }
}
