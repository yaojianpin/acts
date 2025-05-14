mod act;
mod message;
mod proc;
mod scher;
mod state;
mod step;
mod task;
mod tree;
mod vars;
mod workflow;

use super::{Process, Runtime};
use crate::{ConfigData, Engine, EngineBuilder, Signal, Workflow, export::Channel};
use std::sync::Arc;

fn create_proc(workflow: &mut Workflow, pid: &str) -> (Arc<Process>, Arc<Runtime>, Arc<Channel>) {
    let engine = Engine::new().start();
    let rt = engine.runtime();

    let proc = rt.create_proc(pid, workflow);
    let emitter = engine.channel().clone();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let s2 = sig.clone();
    emitter.on_complete(move |p| {
        if p.inner().state().is_completed() {
            s1.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.pid, p.state);
        s2.close();
    });
    (proc, rt, emitter)
}

#[allow(clippy::type_complexity)]
fn create_proc_signal<R: Clone + Default + Sync + Send + 'static>(
    workflow: &mut Workflow,
    pid: &str,
) -> (
    Arc<Process>,
    Arc<Runtime>,
    Arc<crate::export::Channel>,
    Signal<R>,
    Signal<R>,
) {
    let engine = Engine::new().start();
    let rt = engine.runtime();

    let proc = rt.create_proc(pid, workflow);

    let emitter = engine.channel().clone();
    let sig = engine.signal(R::default());
    let rx2 = sig.clone();
    let rx3 = sig.clone();
    emitter.on_complete(move |p| {
        // println!("message: {p:?}");
        if p.state().is_completed() {
            rx2.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        rx3.close();
    });

    (proc, rt, emitter, sig.clone(), sig.clone())
}

#[allow(clippy::type_complexity)]
fn create_proc_signal2<R: Clone + Default + Send + 'static>(
    workflow: &Workflow,
    pid: &str,
) -> (Engine, Arc<Process>, Signal<R>, Signal<R>) {
    let engine = Engine::new().start();
    let rt = engine.runtime();

    let proc = rt.create_proc(pid, workflow);

    let emitter = engine.channel().clone();
    let sig = engine.signal(R::default());
    let rx2 = sig.clone();
    let rx3 = sig.clone();
    emitter.on_complete(move |p| {
        if p.inner().state().is_completed() {
            rx2.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        rx3.close();
    });
    (engine, proc, sig.clone(), sig.clone())
}

#[allow(clippy::type_complexity)]
fn create_proc_signal_config<R: Clone + Default + Send + 'static>(
    config: &ConfigData,
    workflow: &Workflow,
    pid: &str,
) -> (Engine, Arc<Process>, Signal<R>) {
    let engine = EngineBuilder::new().set_config(config).build().start();
    let rt = engine.runtime();

    let proc = rt.create_proc(pid, workflow);

    let emitter = engine.channel().clone();
    let (s1, s2, sig) = engine.signal(R::default()).triple();
    emitter.on_complete(move |p| {
        if p.inner().state().is_completed() {
            s1.close();
        }
    });

    emitter.on_error(move |p| {
        println!("error in '{}', error={}", p.inner().pid, p.inner().state);
        s2.close();
    });
    (engine, proc, sig)
}
