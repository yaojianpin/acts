use crate::{Engine, EngineBuilder, Signal, Workflow, config::ConfigData, scheduler::Process};
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
pub fn create_proc(workflow: &Workflow, pid: &str) -> (Engine, Arc<Process>) {
    let engine = Engine::new().start().unwrap();
    let proc = engine.runtime().create_proc(pid, workflow);
    engine.channel().on_message(|e| println!("message: {e:?}"));
    (engine, proc)
}

/// Like [`create_proc`] but uses [`EngineBuilder`] with a custom [`ConfigData`].
pub(crate) fn create_proc_with_config(
    config: &ConfigData,
    workflow: &Workflow,
    pid: &str,
) -> (Engine, Arc<Process>) {
    let engine = EngineBuilder::new()
        .set_config(config)
        .build()
        .start()
        .unwrap();
    let proc = engine.runtime().create_proc(pid, workflow);
    engine.channel().on_message(|e| println!("message: {e:?}"));

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
        std::thread::sleep(std::time::Duration::from_millis(20));
        s1.close();
    });

    channel.on_error(move |e| {
        println!("on_error: {e:?}");
        std::thread::sleep(std::time::Duration::from_millis(20));
        s2.close();
    });
}
