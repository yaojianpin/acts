use serde::Deserialize;
use std::path::Path;
use toml::Table;

#[derive(Debug, Clone)]
pub struct Config {
    pub data: ConfigData,
    pub table: Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigLog {
    pub dir: String,
    pub level: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigData {
    pub cache_cap: Option<i64>,
    pub tick_interval_secs: Option<i64>,

    // will delete message after the max retries
    // cancel the settings by setting to 0
    pub max_message_retry_times: Option<i32>,
    /// max times a tree node can be executed in one process; protects against
    /// unbounded task creation caused by a node self-loop / cyclic `next`.
    /// 0 disables the check
    pub max_node_run_times: Option<i64>,
    /// Maximum scheduler task lanes; defaults to available parallelism.
    /// Each lane executes one task at a time.
    /// Tasks for the same pid always hash to the same lane to preserve FIFO.
    pub scheduler_workers: Option<usize>,

    // log config
    pub log: Option<ConfigLog>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: ConfigData::default(),
            table: Table::new(),
        }
    }
}

impl Config {
    pub fn create(path: &Path) -> crate::Result<Self> {
        let data = std::fs::read_to_string(path).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to load config file {}: {err}",
                path.display()
            ))
        })?;

        let table = toml::from_str::<Table>(&data).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the toml file({}): {err}",
                path.display()
            ))
        })?;

        let data = ConfigData::deserialize(table.clone()).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the config file({}): {err}",
                path.display()
            ))
        })?;

        Ok(Self { table, data })
    }

    pub fn get<'de, T>(&self, name: &str) -> crate::Result<T>
    where
        T: Deserialize<'de>,
    {
        let value = self
            .table
            .get(name)
            .ok_or_else(|| crate::ActError::Config(format!("config '{name}' does not exist")))?
            .clone();
        T::deserialize(value)
            .map_err(|err| crate::ActError::Config(format!("failed to get '{name}' config: {err}")))
    }

    pub fn has(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    /// Read another acts.toml and deep-merge it over this configuration: a
    /// key present in `path` overrides the base value, nested tables merge
    /// field by field (so an override file can change `[log].dir` while the
    /// `[log].level` from the base is kept), and scalar/array values replace
    /// wholesale. A missing file is a no-op; an unparsable file is an error.
    /// Used by `acts-server` to let local `acts.toml` files override the
    /// `~/.acts` defaults.
    pub fn overlay_file(&mut self, path: &Path) -> crate::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(path).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to load config file {}: {err}",
                path.display()
            ))
        })?;
        let table = toml::from_str::<Table>(&text).map_err(|err| {
            crate::ActError::Config(format!(
                "failed to parse the toml file({}): {err}",
                path.display()
            ))
        })?;
        merge_table(&mut self.table, table);
        self.data = ConfigData::deserialize(self.table.clone()).map_err(|err| {
            crate::ActError::Config(format!("failed to parse the merged config: {err}"))
        })?;
        Ok(())
    }

    pub fn cache_cap(&self) -> i64 {
        self.data.cache_cap.unwrap_or(1024)
    }
    pub fn max_message_retry_times(&self) -> i32 {
        self.data.max_message_retry_times.unwrap_or(20)
    }
    pub fn max_node_run_times(&self) -> i64 {
        self.data.max_node_run_times.unwrap_or(1000)
    }
    pub fn tick_interval_secs(&self) -> i64 {
        self.data.tick_interval_secs.unwrap_or(15)
    }

    /// Maximum concurrently executing scheduler jobs. This is also the explicit
    /// in-flight admission limit for the fixed task-lane pool.
    pub fn scheduler_workers(&self) -> usize {
        let configured = self
            .data
            .scheduler_workers
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
        configured.clamp(1, 1024)
    }

    pub fn log(&self) -> ConfigLog {
        self.data.log.clone().unwrap_or(ConfigLog {
            dir: "log".to_string(),
            level: "INFO".to_string(),
        })
    }
}

/// Merge `over` into `base` in place: nested tables merge field by field,
/// every other value replaces the base value for that key.
fn merge_table(base: &mut Table, over: Table) {
    for (key, value) in over {
        if let Some(existing) = base.get_mut(&key) {
            match (existing, value) {
                (toml::Value::Table(base_table), toml::Value::Table(over_table)) => {
                    merge_table(base_table, over_table);
                }
                (slot, value) => *slot = value,
            }
        } else {
            base.insert(key, value);
        }
    }
}

/// Controls behavior when a snapshot target's key params or data are absent
/// at a task's prepare step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingParamAction {
    /// Silently skip sealing data for this task.
    #[default]
    Skip,
    /// Return an error listing the missing parameters.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per-test scratch dir so parallel runs never collide.
    fn scratch(name: &str) -> std::path::PathBuf {
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
        let dir = std::env::temp_dir().join(format!(
            "acts-config-{}-{}-{}",
            std::process::id(),
            name,
            thread
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn overlay_file_merges_nested_tables_field_by_field() {
        let dir = scratch("merge");
        let base = write(
            &dir,
            "base.toml",
            r#"
cache_cap = 1024
[log]
dir = "data"
level = "INFO"
"#,
        );
        let over = write(
            &dir,
            "over.toml",
            r#"
[log]
dir = "other"
"#,
        );

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&over).unwrap();

        // the override's [log].dir won, the base's level survived
        assert_eq!(config.log().dir, "other");
        assert_eq!(config.log().level, "INFO");
        assert_eq!(config.cache_cap(), 1024);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_file_replaces_scalars_and_adds_new_keys() {
        let dir = scratch("scalar");
        let base = write(
            &dir,
            "base.toml",
            "cache_cap = 1024\n[log]\ndir = \"data\"\nlevel = \"INFO\"\n",
        );
        let over = write(&dir, "over.toml", "cache_cap = 512\n[web]\nport = 10082\n");

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&over).unwrap();

        assert_eq!(config.cache_cap(), 512);
        assert!(config.has("web"));
        assert_eq!(config.log().dir, "data");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_file_missing_is_a_noop_and_unparsable_is_an_error() {
        let dir = scratch("missing");
        let base = write(
            &dir,
            "base.toml",
            "[log]\ndir = \"data\"\nlevel = \"INFO\"\n",
        );

        let mut config = Config::create(&base).unwrap();
        config.overlay_file(&dir.join("nope.toml")).unwrap();
        assert_eq!(config.log().level, "INFO");

        let bad = write(&dir, "bad.toml", "this is not [ valid toml");
        assert!(config.overlay_file(&bad).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_reports_missing_and_invalid_files() {
        let dir = std::env::temp_dir().join(format!("acts-config-create-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(Config::create(&dir.join("missing.toml")).is_err());

        let path = dir.join("invalid.toml");
        std::fs::write(&path, "this is not [ valid toml").unwrap();
        assert!(Config::create(&path).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
