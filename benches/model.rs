//! Hotspot benchmarks for the model layer: JSON Schema compilation and
//! validation of `ActSchema` (a workflow's `inputs` / `exposes`), which the
//! engine validates on every process start and completion.

use acts::{ActSchema, Variant, VariantTypes};
use criterion::*;
use serde_json::{Value as JsonValue, json};

/// A schema of `fields` typed required properties plus a matching instance.
/// `nonce` is part of every property name, so a distinct nonce is a distinct
/// serialized schema — a validator-cache miss.
fn schema_with(fields: usize, nonce: u64) -> (ActSchema, JsonValue) {
    let mut variants = Vec::with_capacity(fields);
    let mut data = serde_json::Map::with_capacity(fields);

    for i in 0..fields {
        let name = format!("f_{}_{}", nonce, i);
        variants.push(
            Variant::new()
                .name(&name)
                .r#type(VariantTypes::Number)
                .required(true),
        );
        data.insert(name, json!(i));
    }

    (ActSchema::Multiple(variants), JsonValue::Object(data))
}

/// Benchmark: `ActSchema::validate`.
///
/// `cached` validates the same schema repeatedly — the production path, where
/// a deployed workflow's inputs/exposes hash to one compiled validator.
/// `compile` validates a schema whose content was never seen before, so the
/// `VALIDATORS` cache misses and `jsonschema::Validator::new` runs every
/// iteration: the compilation cost the cache removes.
fn schema_validate(c: &mut Criterion) {
    let mut group = c.benchmark_group("schema_validate");
    group.throughput(Throughput::Elements(1));
    group.sample_size(10);

    for &fields in &[1usize, 8, 32] {
        let (schema, value) = schema_with(fields, 0);
        group.bench_function(BenchmarkId::new("cached", fields.to_string()), |b| {
            b.iter(|| schema.validate(black_box(&value)).unwrap());
        });
    }

    // The miss path is measured with a short window: each iteration compiles a
    // validator and caches it under a never-reused key (the cache is
    // unbounded), so the total work is capped to keep the process footprint
    // bounded.
    group.measurement_time(std::time::Duration::from_secs(2));
    group.warm_up_time(std::time::Duration::from_millis(500));
    group.bench_function("compile", |b| {
        let mut nonce = 0u64;
        b.iter(|| {
            nonce += 1;
            let (schema, value) = schema_with(8, nonce);
            schema.validate(black_box(&value)).unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, schema_validate);
criterion_main!(benches);
