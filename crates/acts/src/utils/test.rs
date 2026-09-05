use crate::{Config, Engine, Signal, Workflow, scheduler::Process};
use std::sync::Arc;

// Package uses constants
pub const USES_IRQ: &str = "acts.core.irq";
pub const USES_MSG: &str = "acts.core.msg";
pub const USES_SET: &str = "acts.transform.set";
pub const USES_PARALLEL: &str = "acts.core.parallel";
pub const USES_SUBFLOW: &str = "acts.core.subflow";
pub const USES_SEQUENCE: &str = "acts.core.sequence";
pub const USES_ACTION: &str = "acts.core.action";
pub const USES_BLOCK: &str = "acts.core.block";
pub const USES_CODE: &str = "acts.transform.code";

/// Unified test helper. Creates an Engine and a Process from a workflow.
/// Returns `(Engine, Arc<Process>)`.
///
/// Tests should get Runtime via `engine.runtime()`, create signals via
/// `engine.signal()`, and manage their own signal/channel lifecycle.
#[allow(clippy::type_complexity)]
pub async fn create_proc(workflow: &Workflow, pid: &str) -> (Engine, Arc<Process>) {
    let engine = Engine::new().start().await.unwrap();
    let proc = engine.runtime().create_proc(pid, workflow);
    engine
        .channel()
        .on_message(|e| async move { println!("message: {e:?}") });
    (engine, proc)
}

/// Like [`create_proc`] but uses [`EngineBuilder`] with a custom [`ConfigData`].
pub(crate) async fn create_proc_with_config(
    config: &Config,
    workflow: &Workflow,
    pid: &str,
) -> (Engine, Arc<Process>) {
    let engine = Engine::builder()
        .set_config(config)
        .build()
        .start()
        .await
        .unwrap();
    let proc = engine.runtime().create_proc(pid, workflow);
    engine
        .channel()
        .on_message(|e| async move { println!("message: {e:?}") });

    (engine, proc)
}

pub(crate) fn auto_complete<S>(engine: &Engine, sig: &Signal<S>)
where
    S: Clone + Send + Sync + 'static,
{
    let (s1, s2) = sig.double();
    let channel = engine.channel();
    channel.on_complete(move |e| {
        println!("on_complete: {e:?}");
        let s1 = s1.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            s1.close();
        }
    });

    channel.on_error(move |e| {
        println!("on_error: {e:?}");
        let s2 = s2.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            s2.close();
        }
    });
}
