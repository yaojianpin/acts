use crate::{ActPackageDefinition, ActPlugin, ActRunAs, Engine};
use parking_lot::Mutex;
use std::sync::Arc;

use serial_test::serial;
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn plugin_common_register() {
    let test_plugin = TestPlugin::new();
    Engine::builder()
        .add_plugin(&test_plugin)
        .start()
        .await
        .unwrap();
    assert!(*test_plugin.is_init.lock());
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn plugin_package_register() {
    let test_plugin = TestPackagePlugin;
    let engine = Engine::builder()
        .add_plugin(&test_plugin)
        .start()
        .await
        .unwrap();

    engine
        .executor(&crate::Principal::unrestricted())
        .ext()
        .register_package(&ActPackageDefinition {
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
        })
        .await
        .unwrap();
    engine
        .executor(&crate::Principal::unrestricted())
        .ext()
        .register_package(&ActPackageDefinition {
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
        })
        .await
        .unwrap();

    let pack1 = engine
        .executor(&crate::Principal::unrestricted())
        .pack()
        .get("test_package")
        .await
        .unwrap();
    assert_eq!(pack1.run_as, ActRunAs::Irq);

    let pack2 = engine
        .executor(&crate::Principal::unrestricted())
        .pack()
        .get("test_package2")
        .await
        .unwrap();
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
    async fn prepare(&self, engine: &Engine) -> crate::Result<()> {
        println!("TestPlugin");
        *self.is_init.lock() = true;

        // engine.register_module("name", module);
        let emitter = engine.channel();
        emitter.on_start(|_w| async {});
        emitter.on_complete(|_w| async {});

        emitter.on_message(|_msg| async {});

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct TestPackagePlugin;

#[async_trait::async_trait]
impl ActPlugin for TestPackagePlugin {
    async fn prepare(&self, _engine: &Engine) -> crate::Result<()> {
        println!("TestPackagePlugin");
        Ok(())
    }
}

/// Every lifecycle transition a test plugin observed, in order.
#[derive(Debug, Clone, Default)]
struct Events(Arc<Mutex<Vec<&'static str>>>);

impl Events {
    fn push(&self, event: &'static str) {
        self.0.lock().push(event);
    }
}

/// A plugin with a scripted failure at one phase.
#[derive(Debug, Clone)]
struct LifecyclePlugin {
    name: &'static str,
    fail_prepare: bool,
    fail_start: bool,
    events: Events,
}

#[async_trait::async_trait]
impl ActPlugin for LifecyclePlugin {
    async fn prepare(&self, _engine: &Engine) -> crate::Result<()> {
        self.events.push(self.name);
        if self.fail_prepare {
            return Err(crate::ActError::Config(format!(
                "{} prepare failed",
                self.name
            )));
        }
        Ok(())
    }

    async fn start(&self, _engine: &Engine) -> crate::Result<()> {
        self.events.push(match self.name {
            "a" => "start-a",
            _ => "start-b",
        });
        if self.fail_start {
            return Err(crate::ActError::Config(format!(
                "{} start failed",
                self.name
            )));
        }
        Ok(())
    }

    async fn shutdown(&self) -> crate::Result<()> {
        self.events.push(match self.name {
            "a" => "stop-a",
            _ => "stop-b",
        });
        Ok(())
    }
}

/// A plugin's prepare runs for every plugin before any start, and close shuts
/// them down in reverse start order.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn plugins_prepare_then_start_and_shutdown_in_reverse() {
    let events = Events::default();
    let engine = Engine::builder()
        .add_plugin(&LifecyclePlugin {
            name: "a",
            fail_prepare: false,
            fail_start: false,
            events: events.clone(),
        })
        .add_plugin(&LifecyclePlugin {
            name: "b",
            fail_prepare: false,
            fail_start: false,
            events: events.clone(),
        })
        .start()
        .await
        .unwrap();
    assert_eq!(
        *events.0.lock(),
        vec!["a", "b", "start-a", "start-b"],
        "all prepares run before the first start"
    );
    engine.close().await;
    assert_eq!(
        *events.0.lock(),
        vec!["a", "b", "start-a", "start-b", "stop-b", "stop-a"],
        "close joins the plugins in reverse start order"
    );
}

/// A prepare failure starts no listener: no plugin reaches `start`, and the
/// rollback still shuts the plugins down.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_failure_starts_nothing() {
    let events = Events::default();
    let result = Engine::builder()
        .add_plugin(&LifecyclePlugin {
            name: "a",
            fail_prepare: false,
            fail_start: false,
            events: events.clone(),
        })
        .add_plugin(&LifecyclePlugin {
            name: "b",
            fail_prepare: true,
            fail_start: false,
            events: events.clone(),
        })
        .start()
        .await;
    assert!(result.is_err(), "the preparation failure aborts the start");
    assert_eq!(
        *events.0.lock(),
        vec!["a", "b", "stop-b", "stop-a"],
        "no start ran; the rollback stopped both plugins in reverse order"
    );
}

/// A start failure stops the plugins that started, in reverse order, before
/// the start returns its error.
#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn a_start_failure_stops_the_started_plugins_in_reverse() {
    let events = Events::default();
    let result = Engine::builder()
        .add_plugin(&LifecyclePlugin {
            name: "a",
            fail_prepare: false,
            fail_start: false,
            events: events.clone(),
        })
        .add_plugin(&LifecyclePlugin {
            name: "b",
            fail_prepare: false,
            fail_start: true,
            events: events.clone(),
        })
        .start()
        .await;
    assert!(result.is_err(), "the start failure aborts the engine start");
    assert_eq!(
        *events.0.lock(),
        vec!["a", "b", "start-a", "start-b", "stop-b", "stop-a"],
        "the rollback stops every prepared plugin, in reverse order"
    );
}
