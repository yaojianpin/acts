#![cfg(test)]

//! Executable dependency rules for the execution runtime.
//!
//! The runtime's layers are directories, and this test is what keeps them
//! honest: every file of a layer is checked the moment it appears, and the
//! shell is the only place that may touch the engine.
//!
//! - `core/**` is pure policy: no IO, no `tokio`, no storage, no engine
//!   vocabulary, and no dependency on a layer above it;
//! - `durable/**` persists bytes. It may name `crate::store` — but only in the
//!   declared adapter file, so the coupling stays visible and countable — and
//!   never reaches into the shell;
//! - `shell/**` (the async shell) and `scheduler/**` are deliberately
//!   unconstrained: they are the only places allowed to touch `tokio`, the
//!   engine and the store handle.
//!
//! The inventories below are what keeps that readable while `runtime/` is a
//! tree rather than a single module: every entry at the root is classified,
//! and the rule tables are checked against the classification.

use std::{fs, path::PathBuf};

/// The pure-policy module trees, relative to `src/runtime`.
const PURE_MODULES: &[&str] = &["core"];

/// The tier an entry at the runtime root belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// Pure policy: scanned by the pure rules.
    Pure,
    /// Byte-level persistence: scanned by the byte rules.
    Byte,
    /// The async shell, unconstrained by design.
    Shell,
    /// The entry point.
    Root,
}

/// Every entry at the runtime root, with its tier.
///
/// The directory keeps the layering visible; this inventory keeps the
/// *inventory* honest (`every_runtime_entry_is_classified`) and ties the rule
/// tables to it (`the_declared_tiers_agree`) — a new module tree that is not
/// listed would otherwise be scanned by nothing.
const RUNTIME_ROOT: &[(&str, Tier)] = &[
    ("core", Tier::Pure),
    ("durable", Tier::Byte),
    ("shell", Tier::Shell),
    ("scheduler", Tier::Shell),
    ("mod.rs", Tier::Root),
];

/// Every module above the pure and byte layers: naming one of them from a
/// module below would make that module depend on the layer above it.
const SHELL_MODULES: &[&str] = &["crate::runtime::scheduler", "crate::runtime::shell"];

/// `durable/**` files allowed to name `crate::store`, with the reason.
const STORE_ADAPTERS: &[(&str, &str)] = &[(
    "durable/async_document_store.rs",
    "the async repository adapter: the migration boundary onto `crate::store::KvStore`",
)];

/// Names that are engine vocabulary: the pure layer must stay independent of all of them.
const ENGINE_VOCABULARY: &[&str] = &[
    "crate::cache",
    "crate::runtime_state",
    "crate::service",
    "crate::services",
    "crate::store",
    "tokio::",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    Pure,
    Durable,
}

/// The names a file of `layer` may not name.
///
/// `crate::store` is the byte layer's own name, so only the pure layer is
/// forbidden it; everything above the pure layer is forbidden to both, and the
/// byte layer additionally sits below the pure one.
fn forbidden(layer: Layer) -> Vec<&'static str> {
    let vocabulary = ENGINE_VOCABULARY
        .iter()
        .copied()
        .filter(|name| !(layer == Layer::Durable && *name == "crate::store"));
    let below = match layer {
        Layer::Pure => Some("crate::runtime::durable"),
        Layer::Durable => None,
    };
    vocabulary
        .chain(below)
        .chain(SHELL_MODULES.iter().copied())
        .collect()
}

/// `crates/acts/src/runtime` — the directory every rule here is about.
fn runtime_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime")
}

/// Every `.rs` file under `src/runtime/<dir>`, recursively, as
/// `(path relative to src/runtime, source)`.
fn sources(dir: &str) -> Vec<(String, String)> {
    let base = runtime_root();
    let mut files = Vec::new();
    collect(&base.join(dir), &base, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn collect(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, String)>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", dir.display()));
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, base, out);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        // Skip this file and the type-surface suite: both are test-only, and
        // they are the only places that name the forbidden strings by design.
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if name == "architecture.rs" || name == "coverage.rs" {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} must be valid UTF-8: {error}", path.display()));
        let relative = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((relative, source));
    }
}

fn violations(layer: Layer) -> Vec<String> {
    let forbidden = forbidden(layer);
    let adapters = STORE_ADAPTERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let dirs: &[&str] = match layer {
        Layer::Pure => PURE_MODULES,
        Layer::Durable => &["durable"],
    };
    dirs.iter()
        .flat_map(|dir| sources(dir))
        // A declared adapter is the boundary itself: it is the one place the
        // engine vocabulary is allowed to appear, and `crate::store` — the
        // only name it must be allowed to carry — is checked by
        // `store_coupling_is_explicit_and_minimal` instead.
        .filter(|(name, _)| !adapters.contains(&name.as_str()))
        .flat_map(|(name, source)| {
            forbidden
                .iter()
                .filter(move |dependency| source.contains(**dependency))
                .map(move |dependency| format!("{name}: forbidden dependency `{dependency}`"))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn pure_modules_have_no_infrastructure_dependencies() {
    let violations = violations(Layer::Pure);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn durable_layer_stays_out_of_the_engine_and_the_shell() {
    let violations = violations(Layer::Durable);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// The `crate::store` coupling is confined to the declared adapters: every
/// other `durable` file must not name it, and every declared adapter must
/// actually use it (a stale whitelist entry is a lie).
#[test]
fn store_coupling_is_explicit_and_minimal() {
    let declared = STORE_ADAPTERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    let sources = sources("durable");

    let leaked = sources
        .iter()
        .filter(|(name, source)| {
            source.contains("crate::store") && !declared.contains(&name.as_str())
        })
        .map(|(name, _)| format!("{name}: names `crate::store` without being a declared adapter"))
        .collect::<Vec<_>>();
    assert!(leaked.is_empty(), "{}", leaked.join("\n"));

    for (adapter, reason) in STORE_ADAPTERS {
        let source = sources
            .iter()
            .find_map(|(name, source)| (name == adapter).then_some(source))
            .unwrap_or_else(|| panic!("declared adapter `{adapter}` does not exist ({reason})"));
        assert!(
            source.contains("crate::store"),
            "declared adapter `{adapter}` no longer names `crate::store` — drop the whitelist entry ({reason})"
        );
    }
}

/// Every entry at the runtime root is classified. A new module tree shows up as
/// this failure instead of as a layer nothing scans.
#[test]
fn every_runtime_entry_is_classified() {
    let mut actual: Vec<String> = fs::read_dir(runtime_root())
        .expect("src/runtime must be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    actual.sort();
    let mut declared: Vec<String> = RUNTIME_ROOT
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    declared.sort();
    assert_eq!(
        actual, declared,
        "classify every entry at src/runtime in RUNTIME_ROOT: an unlisted module tree is scanned by nothing"
    );
}

/// The declared tiers agree with the rule tables, so neither the pure list nor
/// the forbidden shell names can drift from the inventory.
#[test]
fn the_declared_tiers_agree() {
    let mut pure: Vec<&str> = RUNTIME_ROOT
        .iter()
        .filter(|(_, tier)| *tier == Tier::Pure)
        .map(|(name, _)| *name)
        .collect();
    pure.sort();
    let mut declared: Vec<&str> = PURE_MODULES.to_vec();
    declared.sort();
    assert_eq!(pure, declared, "PURE_MODULES and RUNTIME_ROOT disagree");

    let mut shell: Vec<String> = RUNTIME_ROOT
        .iter()
        .filter(|(_, tier)| *tier == Tier::Shell)
        .map(|(name, _)| format!("crate::runtime::{}", name.trim_end_matches(".rs")))
        .collect();
    shell.sort();
    let mut forbidden: Vec<String> = SHELL_MODULES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    forbidden.sort();
    assert_eq!(
        shell, forbidden,
        "SHELL_MODULES and the shell entries of RUNTIME_ROOT disagree"
    );
}
