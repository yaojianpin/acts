//! Engine construction for the acts server binary — shared with the
//! integration tests so they exercise exactly what `acts-server` runs.

use acts::{Config, Engine, KvStore, MissingParamAction, SnapshotOptions, SnapshotPolicy};
use serde::Deserialize;
use std::sync::Arc;

/// Which plugins the built engine registers.
#[derive(Debug, Clone, Default)]
pub struct ServerPlugins {
    /// gRPC server plugin (default port 10080 unless `[grpc]` sets one).
    pub grpc: bool,
    /// web plugin.
    pub web: bool,
    /// NATS plugin — only registered when the config has a `[nats]` section.
    pub nats: bool,
}

impl ServerPlugins {
    /// Everything the shipped `acts-server` binary registers.
    pub fn full() -> Self {
        Self {
            grpc: true,
            web: true,
            nats: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DbConfig {
    pub database_url: String,
}

/// One `[[snapshot]]` target entry from the engine config file.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotConfig {
    /// target name — the sealed-data name the scheduler injects into task
    /// env as `$<name>` (e.g. `profile`, `secrets`).
    pub name: String,
    /// when the cache value is frozen into task sealed data
    /// (`per_proc` default: once per task lineage; `per_task`: every task
    /// re-reads the latest value at its prepare).
    #[serde(default)]
    pub policy: SnapshotPolicy,
    /// task param names whose values join with `/` into this target's scope
    /// key. Empty: one global scope per target.
    #[serde(default)]
    pub scope: Vec<String>,
    /// what happens when a key param or the snapshot data is absent at a
    /// task's prepare (`skip` default: seal nothing; `error`: fail the task).
    #[serde(default)]
    pub on_missing: MissingParamAction,
    /// how long an entry stays valid after its last refresh — a suffixed
    /// duration like `"2s"`, `"5m"`, `"3h"`, `"1d"`, or a bare number of
    /// seconds; unset never expires. Expired entries are dropped on read
    /// and by a periodic sweep.
    #[serde(default, deserialize_with = "de_ttl_secs")]
    pub ttl: Option<u64>,
}

impl From<SnapshotConfig> for SnapshotOptions {
    fn from(c: SnapshotConfig) -> Self {
        Self {
            policy: c.policy,
            scope: c.scope,
            on_missing: c.on_missing,
            ttl_secs: c.ttl,
        }
    }
}

/// Parse a suffixed TTL like `2s`, `5m`, `3h`, `1d` into seconds.
fn parse_ttl_secs(text: &str) -> Result<u64, String> {
    let (factor, digits) = match text.as_bytes().last() {
        Some(b's') => (1, &text[..text.len() - 1]),
        Some(b'm') => (60, &text[..text.len() - 1]),
        Some(b'h') => (3600, &text[..text.len() - 1]),
        Some(b'd') => (86_400, &text[..text.len() - 1]),
        _ => {
            return Err(format!(
                "invalid ttl '{text}': expected a suffixed duration like \
                 \"2s\", \"5m\", \"3h\" or \"1d\", or a bare number of seconds"
            ));
        }
    };
    let n: u64 = digits.parse().map_err(|_| {
        format!("invalid ttl '{text}': the value before the unit must be a whole number")
    })?;
    Ok(n * factor)
}

/// Deserialize a snapshot `ttl` config value: suffixed duration string
/// (`2s`/`5m`/`3h`/`1d`) or a bare integer counting seconds.
fn de_ttl_secs<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct TtlVisitor;

    impl serde::de::Visitor<'_> for TtlVisitor {
        type Value = Option<u64>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(
                "a suffixed duration (\"2s\", \"5m\", \"3h\", \"1d\") or a \
                 bare number of seconds",
            )
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            u64::try_from(v)
                .map(Some)
                .map_err(|_| E::custom("ttl must be a non-negative number"))
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            parse_ttl_secs(v).map(Some).map_err(E::custom)
        }
    }

    deserializer.deserialize_any(TtlVisitor)
}

/// Build the acts-server engine: core packages plus the transport plugins
/// selected by `plugins`. The NATS plugin is additionally gated on the
/// config having a `[nats]` section, so a server without one never tries to
/// reach a broker. `[[snapshot]]` targets declared in the config are
/// pre-registered with [`acts::EngineBuilder::add_snapshot`] so the
/// scheduler seals their values under the configured policy/scope.
pub fn build_engine(config: &Config, store: Arc<dyn KvStore>, plugins: &ServerPlugins) -> Engine {
    let mut builder = Engine::builder().set_config(config).set_store(store);
    if config.has("snapshot") {
        let targets = config
            .get::<Vec<SnapshotConfig>>("snapshot")
            .expect("config 'snapshot' must be a [[snapshot]] array of target tables");
        for target in &targets {
            builder = builder.add_snapshot(&target.name, target.clone().into());
        }
    }
    if plugins.grpc && config.has("grpc") {
        builder = builder.add_plugin(&acts_plugin_grpc::GrpcPlugin::new());
    }
    if plugins.web && config.has("web") {
        builder = builder.add_plugin(&acts_plugin_web::WebPlugin::new());
    }
    builder = builder
        .add_package::<acts_package_http::HttpPackage>()
        .add_package::<acts_package_shell::ShellPackage>()
        .add_package::<acts_package_state::StatePackage>()
        .add_package::<acts_package_nats::NatsPackage>();
    if plugins.nats && config.has("nats") {
        builder = builder.add_plugin(&acts_plugin_nats::NatsPlugin::new());
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts::MemoryStore;

    fn write_config(body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let thread = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let dir =
            std::env::temp_dir().join(format!("acts-snap-cfg-{}-{thread}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("acts.toml");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn snapshot_config_parses_and_maps_to_options() {
        let (dir, path) = write_config(
            r#"
[[snapshot]]
name = "profile"
policy = "per_proc"

[[snapshot]]
name = "secrets"
policy = "per_task"
scope = ["unit"]
on_missing = "error"
ttl = "5m"
"#,
        );
        let config = Config::create(&path);

        // build_engine accepts a [[snapshot]] config and pre-registers the
        // targets through EngineBuilder::add_snapshot (a parse or mapping
        // error would panic here).
        build_engine(
            &config,
            Arc::new(MemoryStore::new()),
            &ServerPlugins {
                grpc: false,
                web: false,
                nats: false,
            },
        );

        // parsed entries translate 1:1 into snapshot registration options
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].name, "profile");
        assert_eq!(targets[0].policy, SnapshotPolicy::PerProc);
        assert_eq!(targets[1].name, "secrets");
        assert_eq!(targets[1].policy, SnapshotPolicy::PerTask);
        assert_eq!(targets[1].scope, vec!["unit".to_string()]);
        assert_eq!(targets[1].on_missing, MissingParamAction::Error);
        assert_eq!(targets[1].ttl, Some(300));

        let options: SnapshotOptions = targets[1].clone().into();
        assert_eq!(options.policy, SnapshotPolicy::PerTask);
        assert_eq!(options.scope, vec!["unit".to_string()]);
        assert_eq!(options.on_missing, MissingParamAction::Error);
        assert_eq!(options.ttl_secs, Some(300));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn snapshot_config_defaults_fill_missing_fields() {
        let (dir, path) = write_config("[[snapshot]]\nname = \"profile\"\n");
        let config = Config::create(&path);
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets[0].name, "profile");
        assert_eq!(targets[0].policy, SnapshotPolicy::PerProc);
        assert_eq!(targets[0].scope, Vec::<String>::new());
        assert_eq!(targets[0].on_missing, MissingParamAction::Skip);
        assert_eq!(targets[0].ttl, None);

        let options: SnapshotOptions = targets[0].clone().into();
        assert_eq!(options.policy, SnapshotPolicy::PerProc);
        assert_eq!(options.scope, Vec::<String>::new());
        assert_eq!(options.on_missing, MissingParamAction::Skip);
        assert_eq!(options.ttl_secs, None);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn snapshot_config_ttl_duration_units() {
        // suffixed durations parse to seconds
        for (input, want) in [
            ("2s", 2),
            ("5m", 300),
            ("3h", 10_800),
            ("1d", 86_400),
            ("10d", 864_000),
        ] {
            assert_eq!(parse_ttl_secs(input).unwrap(), want, "ttl {input}");
        }
        // a bare integer in the config counts seconds
        let (dir, path) = write_config("[[snapshot]]\nname = \"cron\"\nttl = 3600\n");
        let config = Config::create(&path);
        let targets = config.get::<Vec<SnapshotConfig>>("snapshot").unwrap();
        assert_eq!(targets[0].ttl, Some(3600));
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        // malformed values fail loudly instead of silently disabling ttl
        for bad in ["5x", "", "1.5h", "abc", "-2s"] {
            assert!(parse_ttl_secs(bad).is_err(), "ttl {bad} should be rejected");
        }
    }
}
