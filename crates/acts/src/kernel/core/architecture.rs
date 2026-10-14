#![cfg(test)]

//! Executable dependency rules for the deterministic execution kernel.

use std::{fs, path::PathBuf};

const INFRASTRUCTURE_ADAPTERS: &[&str] = &["async_document_store.rs"];
const TEST_MODULES: &[&str] = &["architecture.rs", "coverage.rs"];
const FORBIDDEN_CORE_DEPENDENCIES: &[&str] = &[
    "crate::cache",
    "crate::runtime",
    "crate::runtime_state",
    "crate::scheduler",
    "crate::service",
    "crate::services",
    "crate::store",
    "tokio::",
];

fn production_sources() -> Vec<(String, String)> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/kernel");
    let mut sources = fs::read_dir(directory)
        .expect("kernel directory must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("rs"))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            (!TEST_MODULES.contains(&name.as_str())).then(|| {
                let source = fs::read_to_string(path).expect("kernel source must be valid UTF-8");
                (name, source)
            })
        })
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

#[test]
fn deterministic_core_has_no_infrastructure_dependencies() {
    let violations = production_sources()
        .into_iter()
        .filter(|(name, _)| !INFRASTRUCTURE_ADAPTERS.contains(&name.as_str()))
        .flat_map(|(name, source)| {
            FORBIDDEN_CORE_DEPENDENCIES
                .iter()
                .filter(move |dependency| source.contains(**dependency))
                .map(move |dependency| format!("{name}: forbidden dependency `{dependency}`"))
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "{}",
        violations.join(
            "
"
        )
    );
}

#[test]
fn infrastructure_exceptions_are_explicit_and_minimal() {
    let sources = production_sources();
    for adapter in INFRASTRUCTURE_ADAPTERS {
        let source = sources
            .iter()
            .find_map(|(name, source)| (name == adapter).then_some(source))
            .unwrap_or_else(|| panic!("declared adapter `{adapter}` does not exist"));
        assert!(source.contains("crate::store"));
    }
}
