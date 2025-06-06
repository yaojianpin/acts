//! Acts postgres store

#![allow(rustdoc::bare_urls)]
// #![doc = include_str!("../README.md")]

mod config;
mod package;

#[cfg(test)]
mod tests;

use acts::{ActError, ActPackage, ActPlugin, ChannelOptions, Result};
use package::StatePackage;

const CONFIG_NAME: &str = "state";
#[derive(Clone)]
pub struct StatePackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for StatePackagePlugin {
    async fn on_init(&self, engine: &acts::Engine) -> Result<()> {
        if !engine.config().has(CONFIG_NAME) {
            println!(
                "skip the initialization of StatePackagePlugin for no 'state' secion in config file"
            );
            return Ok(());
        }
        let config = engine
            .config()
            .get::<config::StateConfig>(CONFIG_NAME)
            .map_err(|err| acts::ActError::Config(format!("get state config error: {}", err)))?;

        let mut client = redis::Client::open(config.database_uri.as_str())
            .map_err(|err| acts::ActError::Config(format!("create redis client error: {}", err)))?;

        redis::cmd("PING")
            .exec(&mut client)
            .map_err(|err| acts::ActError::Config(format!("ping redis error: {}", err)))?;

        let meta = package::StatePackage::meta();
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
            // check the params in inputs
            let Some(params) = e.inputs.get::<serde_json::Value>("params") else {
                executor
                    .act()
                    .error(
                        &e.pid,
                        &e.tid,
                        &ActError::Package("missing 'params' in inputs".to_string()).into(),
                    )
                    .unwrap();
                return;
            };

            // convert the params to StatePackage
            let pakage: StatePackage = serde_json::from_value(params).unwrap();
            match pakage.run(&client, &e.pid) {
                Ok(ref vars) => {
                    executor.act().complete(&e.pid, &e.tid, vars).unwrap();
                }
                Err(err) => {
                    executor.act().error(&e.pid, &e.tid, &err.into()).unwrap();
                }
            }
        });
        Ok(())
    }
}
