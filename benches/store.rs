use acts::{
    MemoryStore, Store,
    data::Proc,
    query::{Expr, Filter, Query},
};
use criterion::async_executor::FuturesExecutor;
use criterion::*;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
}

fn memory_store() -> Arc<Store> {
    Arc::new(Store::new(Arc::new(MemoryStore::new())))
}

fn make_proc(id: &str, mid: &str, i: u64) -> Proc {
    Proc {
        id: id.to_string(),
        name: format!("bench-{}", i),
        mid: mid.to_string(),
        state: "running".to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: now_micros(),
        model: "{}".to_string(),
        env: "{}".to_string(),
        err: None,
        v: 0,
    }
}

/// Benchmark: create Proc records — measures write QPS.
fn store_create_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_create");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc", batch_size.to_string()), |b| {
            b.to_async(FuturesExecutor).iter_custom(move |iters| async move {
                let store = memory_store();
                let procs = store.procs();
                let mid = format!("m_{}", batch_size);
                let prefix = format!("p_{}", batch_size);
                let mut ids = Vec::with_capacity(iters as usize);

                let start = std::time::Instant::now();
                for i in 0..iters {
                    let id = format!("{}_{}", prefix, i);
                    ids.push(id.clone());
                    procs.create(&make_proc(&id, &mid, i)).unwrap();
                }
                // Cleanup
                for id in &ids {
                    procs.delete(id).unwrap();
                }
                start.elapsed()
            })
        });
    }
    group.finish();
}

/// Benchmark: find records by ID — measures random-read QPS.
fn store_find_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_find");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc", batch_size.to_string()), |b| {
            let store = memory_store();
            let procs = store.procs();
            let mid = format!("m_find_{}", batch_size);
            let prefix = format!("p_find_{}", batch_size);
            let mut ids = Vec::with_capacity(batch_size);

            // Pre-create records
            for i in 0..batch_size {
                let id = format!("{}_{}", prefix, i);
                ids.push(id.clone());
                procs.create(&make_proc(&id, &mid, i as u64)).unwrap();
            }

            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let ids = ids.clone();
                let procs = procs.clone();
                async move {
                    let start = std::time::Instant::now();
                    for i in 0..iters {
                        let idx = (i as usize) % ids.len();
                        let _ = procs.find(&ids[idx]).unwrap();
                    }
                    // Cleanup
                    for id in &ids {
                        procs.delete(id).unwrap();
                    }
                    start.elapsed()
                }
            });
        });
    }
    group.finish();
}

/// Benchmark: query records with index filter — measures indexed-read QPS.
fn store_query_index_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_query_index");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc_by_mid", batch_size.to_string()), |b| {
            let store = memory_store();
            let procs = store.procs();
            let mid = format!("m_qidx_{}", batch_size);
            let mut ids = Vec::with_capacity(batch_size);

            // Pre-create records with indexed field
            for i in 0..batch_size {
                let id = format!("qidx_{}_{}", batch_size, i);
                ids.push(id.clone());
                procs.create(&make_proc(&id, &mid, i as u64)).unwrap();
            }

            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let procs = procs.clone();
                let mid = mid.clone();
                let ids = ids.clone();
                async move {
                    let q = Query::new()
                        .filter(Filter::and().expr(Expr::eq("mid", mid)))
                        .limit(10);
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let _ = procs.query(&q).unwrap();
                    }
                    // Cleanup
                    for id in &ids {
                        procs.delete(id).unwrap();
                    }
                    start.elapsed()
                }
            });
        });
    }
    group.finish();
}

/// Benchmark: query records with non-index filter — measures full-scan QPS.
fn store_query_scan_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_query_scan");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc_by_name", batch_size.to_string()), |b| {
            let store = memory_store();
            let procs = store.procs();
            let mid = format!("m_qscan_{}", batch_size);
            let mut ids = Vec::with_capacity(batch_size);

            for i in 0..batch_size {
                let id = format!("qscan_{}_{}", batch_size, i);
                ids.push(id.clone());
                procs.create(&make_proc(&id, &mid, i as u64)).unwrap();
            }

            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let procs = procs.clone();
                let ids = ids.clone();
                async move {
                    // Use 'name' filter (non-indexed field) to trigger full scan
                    let q = Query::new()
                        .filter(Filter::and().expr(Expr::eq("name", "bench-0")))
                        .limit(10);
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let _ = procs.query(&q).unwrap();
                    }
                    // Cleanup
                    for id in &ids {
                        procs.delete(id).unwrap();
                    }
                    start.elapsed()
                }
            });
        });
    }
    group.finish();
}

/// Benchmark: update records — measures write+index-update QPS.
fn store_update_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_update");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc", batch_size.to_string()), |b| {
            let store = memory_store();
            let procs = store.procs();
            let mid = format!("m_upd_{}", batch_size);
            let mut ids = Vec::with_capacity(batch_size);

            // Pre-create records
            for i in 0..batch_size {
                let id = format!("upd_{}_{}", batch_size, i);
                ids.push(id.clone());
                procs.create(&make_proc(&id, &mid, i as u64)).unwrap();
            }

            let update_mid = format!("m_upd_new_{}", batch_size);
            b.to_async(FuturesExecutor).iter_custom(move |iters| {
                let procs = procs.clone();
                let ids = ids.clone();
                let update_mid = update_mid.clone();
                async move {
                    let start = std::time::Instant::now();
                    for i in 0..iters {
                        let idx = (i as usize) % ids.len();
                        let mut proc = procs.find(&ids[idx]).unwrap();
                        proc.state = "completed".to_string();
                        proc.mid = update_mid.clone();
                        procs.update(&proc).unwrap();
                    }
                    // Cleanup
                    for id in &ids {
                        procs.delete(id).unwrap();
                    }
                    start.elapsed()
                }
            });
        });
    }
    group.finish();
}

/// Benchmark: delete records — measures delete QPS.
fn store_delete_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_delete");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(BenchmarkId::new("proc", batch_size.to_string()), |b| {
            b.to_async(FuturesExecutor).iter_custom(move |iters| async move {
                let store = memory_store();
                let procs = store.procs();
                let mid = format!("m_del_{}", batch_size);

                // Pre-create records for this iteration
                let mut ids = Vec::with_capacity(iters as usize);
                for i in 0..iters {
                    let id = format!("del_{}_{}", batch_size, i);
                    ids.push(id.clone());
                    procs.create(&make_proc(&id, &mid, i)).unwrap();
                }

                let start = std::time::Instant::now();
                for id in &ids {
                    procs.delete(id).unwrap();
                }
                start.elapsed()
            })
        });
    }
    group.finish();
}

/// Benchmark: mixed workload — create + find + update + find + delete.
fn store_mixed_qps(c: &mut Criterion) {
    let mut group = c.benchmark_group("store_mixed");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &batch_size in &[100, 1000] {
        group.bench_function(
            BenchmarkId::new("proc_crud", batch_size.to_string()),
            |b| {
                b.to_async(FuturesExecutor).iter_custom(move |iters| async move {
                    let store = memory_store();
                    let procs = store.procs();
                    let mid = format!("m_mix_{}", batch_size);

                    let start = std::time::Instant::now();
                    for i in 0..iters {
                        let id = format!("mix_{}_{}", batch_size, i);
                        // Create
                        procs.create(&make_proc(&id, &mid, i)).unwrap();
                        // Find
                        let _ = procs.find(&id).unwrap();
                        // Update
                        let mut p = procs.find(&id).unwrap();
                        p.state = "completed".to_string();
                        procs.update(&p).unwrap();
                        // Find again
                        let _ = procs.find(&id).unwrap();
                        // Delete
                        procs.delete(&id).unwrap();
                    }
                    start.elapsed()
                })
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    store_create_qps,
    store_find_qps,
    store_query_index_qps,
    store_query_scan_qps,
    store_update_qps,
    store_delete_qps,
    store_mixed_qps,
);
criterion_main!(benches);
