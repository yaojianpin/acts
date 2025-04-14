use acts::{Engine, MessageState, Vars, Workflow};
use criterion::*;
use std::sync::{Arc, Mutex};
use tokio::{
    runtime::Runtime,
    time::{Duration, Instant},
};

fn load(c: &mut Criterion) {
    c.bench_function("load", |b| {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let text = include_str!("./start.yml");
            b.iter(move || {
                Workflow::from_yml(text).unwrap();
            })
        });
    });
}

fn deploy(c: &mut Criterion) {
    c.bench_function("deploy", |b| {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let engine = Engine::new();
            let text = include_str!("./start.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            b.iter(move || {
                engine.executor().model().deploy(&workflow).unwrap();
            })
        });
    });
}

fn start(c: &mut Criterion) {
    c.bench_function("start", |b| {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let engine = Engine::new();
            let text = include_str!("./start.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            engine.executor().model().deploy(&workflow).unwrap();
            b.iter(move || {
                engine
                    .executor()
                    .proc()
                    .start(&workflow.id, &Vars::new())
                    .unwrap();
            })
        });
    });
}

fn act(c: &mut Criterion) {
    c.bench_function("act", |b| {
        let rt = Runtime::new().unwrap();

        b.to_async(rt).iter_custom(|iters| async move {
            // println!("act: iters={iters}");
            let engine = Engine::new();

            let (s, sig) = engine.signal(()).double();
            let text = include_str!("./act.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            engine.executor().model().deploy(&workflow).unwrap();

            let time = Arc::new(Mutex::new(Duration::new(0, 0)));
            let count = Arc::new(Mutex::new(0));
            let t = time.clone();
            let e2 = engine.clone();
            let chan = engine.channel();
            chan.on_message(move |e| {
                if e.is_key("act1") && e.is_state(MessageState::Created) {
                    let start = Instant::now();
                    engine
                        .executor()
                        .act()
                        .complete(&e.pid, &e.tid, &Vars::new())
                        .unwrap();
                    let elapsed = start.elapsed();
                    *t.lock().unwrap() += elapsed;

                    let mut count = count.lock().unwrap();
                    *count += 1;
                    if *count >= iters {
                        s.close();
                    }
                }
            });

            for _ in 0..iters {
                let _ = e2
                    .executor()
                    .proc()
                    .start(&workflow.id, &Vars::new())
                    .unwrap();
            }
            sig.recv().await;
            let time = time.lock().unwrap();
            *time
        })
    });
}

criterion_group!(benches, load, deploy, start, act);
criterion_main!(benches);
