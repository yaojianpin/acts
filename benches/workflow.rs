use acts::{Engine, Vars, Workflow};
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
            let engine = Engine::new();
            engine.start();
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
            engine.start();
            let text = include_str!("./start.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            b.iter(move || {
                engine.manager().deploy(&workflow).unwrap();
            })
        });
    });
}

fn start(c: &mut Criterion) {
    c.bench_function("start", |b| {
        let rt = Runtime::new().unwrap();
        rt.block_on(async move {
            let engine = Engine::new();
            engine.start();
            let text = include_str!("./start.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            engine.manager().deploy(&workflow).unwrap();
            let e = engine.clone();
            b.iter(move || {
                let exec = e.executor();
                exec.start(&workflow.id, &Vars::new()).unwrap();
            })
        });
    });
}

fn act(c: &mut Criterion) {
    c.bench_function("act", |b| {
        let rt = Runtime::new().unwrap();
        b.to_async(rt).iter_custom(|iters| async move {
            // println!("iters: {iters}");
            let engine = Engine::new();
            engine.start();
            let text = include_str!("./act.yml");
            let workflow = Workflow::from_yml(text).unwrap();
            engine.manager().deploy(&workflow).unwrap();

            let exec = engine.executor().clone();
            let time = Arc::new(Mutex::new(Duration::new(0, 0)));
            let count = Arc::new(Mutex::new(0));
            let t = time.clone();
            engine.emitter().on_message(move |e| {
                if e.is_key("act1") && e.is_state("created") {
                    let start = Instant::now();
                    exec.complete(&e.proc_id, &e.id, &Vars::new()).unwrap();
                    let elapsed = start.elapsed();
                    *t.lock().unwrap() += elapsed;

                    let mut count = count.lock().unwrap();
                    *count += 1;
                    // println!("message: pid={} key={} count={}", e.proc_id, e.key, *count);
                    if *count >= iters {
                        // println!("close");
                        e.close();
                    }
                }
            });

            for _ in 0..iters {
                let _ = engine.executor().start(&workflow.id, &Vars::new()).unwrap();
                // println!("start: {}", ret.outputs());
            }

            engine.eloop().await;

            let time = time.lock().unwrap();
            // println!("closed: {}", time.as_millis());
            *time
        })
    });
}

criterion_group!(benches, load, deploy, start, act);
criterion_main!(benches);
