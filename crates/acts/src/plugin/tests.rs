use crate::{ActPlugin, ActRunAs, Engine};
use std::sync::{Arc, Mutex};

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn plugin_common_register() {
    let test_plugin = TestPlugin::new();
    Engine::new().add_plugin(&test_plugin).start().unwrap();
    assert!(*test_plugin.is_init.lock().unwrap());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn plugin_package_register() {
    let test_plugin = TestPackagePlugin;
    let engine = Engine::new().add_plugin(&test_plugin).start().unwrap();
    let pack1 = engine.executor().pack().get("test_package").unwrap();
    assert_eq!(pack1.run_as, ActRunAs::Irq);

    let pack2 = engine.executor().pack().get("test_package2").unwrap();
    assert_eq!(pack2.run_as, ActRunAs::Msg);
}

#[derive(Debug, Clone)]
struct TestPlugin {
    is_init: Arc<Mutex<bool>>,
}

impl TestPlugin {
    fn new() -> Self {
        Self {
            is_init: Arc::new(Mutex::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl ActPlugin for TestPlugin {
    fn on_init(&self, engine: &Engine) -> crate::Result<()> {
        println!("TestPlugin");
        *self.is_init.lock().unwrap() = true;

        // engine.register_module("name", module);
        let emitter = engine.channel();
        emitter.on_start(|_w| {});
        emitter.on_complete(|_w| {});

        emitter.on_message(|_msg| {});

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct TestPackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for TestPackagePlugin {
    fn on_init(&self, engine: &Engine) -> crate::Result<()> {
        println!("TestPackagePlugin");

        engine
            .extender()
            .register_package(&crate::ActPackageDefinition {
                id: "test_package",
                name: "test_package",
                desc: "test package description",
                icon: "test_package_icon",
                doc: "test package doc",
                version: "0.1.0",
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "v1": { "type": "number" }
                    }
                }),
                options: Some(serde_json::json!({
                    "v1": {
                        "ui:widget": "text",
                    }
                })),
                run_as: crate::ActRunAs::Irq,
                resources: vec![],
                catalog: crate::ActPackageCatalog::App,
            })?;

        engine
            .extender()
            .register_package(&crate::ActPackageDefinition {
                id: "test_package2",
                name: "test_package2",
                desc: "test package description",
                icon: "test_package_icon",
                doc: "test package doc",
                version: "0.1.0",
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "v1": { "type": "number" }
                    }
                }),
                options: Some(serde_json::json!({
                    "v1": {
                        "ui:widget": "text",
                    }
                })),
                run_as: crate::ActRunAs::Msg,
                resources: vec![],
                catalog: crate::ActPackageCatalog::App,
            })?;
        Ok(())
    }
}
