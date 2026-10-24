use super::CommandRunner as Command;
use crate::client;
use acts_channel::{Vars, model::SystemStatus};
use clap::Args;

#[derive(Debug, Args)]
#[command(about = "show the server's status: memory, cpu, storage, processes, health and config")]
pub struct StatusArgs {}

/// Human-readable bytes; the raw number stays beside it, so a script can
/// parse the answer while a human reads it.
fn bytes(value: u64) -> String {
    const UNIT: f64 = 1024.0;
    if (value as f64) < UNIT {
        return format!("{value} B");
    }
    let mut size = value as f64;
    for name in ["KB", "MB", "GB", "TB", "PB"] {
        size /= UNIT;
        if size < UNIT {
            return format!("{size:.1} {name} ({value} B)");
        }
    }
    format!("{:.1} EB ({value} B)", size / UNIT)
}

pub async fn process(parent: &mut Command<'_>) -> anyhow::Result<()> {
    let ret = show(parent).await?;
    parent.output(&ret);
    Ok(())
}

/// The status answer as one rendered block: every section the server
/// reports, plus the elapsed time the other commands also carry.
pub async fn show(parent: &mut Command<'_>) -> anyhow::Result<String> {
    let resp = parent
        .send::<SystemStatus>("sys:status", Vars::new())
        .await?;
    let status = client::payload("sys:status", resp.data)?;

    let mut ret = render(&status);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));
    Ok(ret)
}

/// The status answer as one rendered block.
fn render(status: &SystemStatus) -> String {
    let mut ret = String::new();

    ret.push_str(&format!("version      {}\n", status.version));
    ret.push_str(&format!("health       {}", status.health.state));
    let degraded = status
        .health
        .loops
        .iter()
        .filter(|loop_status| loop_status.state != "healthy")
        .map(|loop_status| loop_status.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if !degraded.is_empty() {
        ret.push_str(&format!(" (degraded loops: {degraded})"));
    }
    ret.push('\n');

    ret.push_str(&format!(
        "processes    total {}, active {}\n",
        status.processes.total, status.processes.active
    ));
    ret.push_str(&format!(
        "memory       rss {}, system total {}, available {}\n",
        bytes(status.memory.rss_bytes),
        bytes(status.memory.system_total_bytes),
        bytes(status.memory.system_available_bytes)
    ));
    ret.push_str(&format!(
        "cpu          process {:.1}%, system {:.1}% ({} cores)\n",
        status.cpu.process_percent, status.cpu.system_percent, status.cpu.logical_cores
    ));
    match status.storage.used_bytes {
        Some(used) => ret.push_str(&format!(
            "storage      {}, used {}\n",
            status.storage.kind,
            bytes(used)
        )),
        None => ret.push_str(&format!(
            "storage      {}, size unknown\n",
            status.storage.kind
        )),
    }

    for loop_status in &status.health.loops {
        ret.push_str(&format!(
            "  loop       {:<10} {:<8} failures {} backoff {} ticks\n",
            loop_status.name,
            loop_status.state,
            loop_status.consecutive_failures,
            loop_status.backoff_ticks
        ));
    }

    // The config came from the server's own acts.toml; a human-readable
    // dump is the point, so one unrenderable value is reported instead of
    // failing the whole answer.
    match serde_json::to_string_pretty(&status.config) {
        Ok(config) => {
            for (index, line) in config.lines().enumerate() {
                if index == 0 {
                    ret.push_str("config       ");
                } else {
                    ret.push_str("             ");
                }
                ret.push_str(line);
                ret.push('\n');
            }
        }
        Err(err) => ret.push_str(&format!("config       (unreadable: {err})\n")),
    }

    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use acts_channel::model::{
        CpuStatus, HealthStatus, LoopStatus, MemoryStatus, ProcessCounts, StorageStatus,
    };
    use serde_json::json;

    fn sample() -> SystemStatus {
        SystemStatus {
            version: "0.25.0".to_string(),
            memory: MemoryStatus {
                rss_bytes: 15_728_640,
                system_total_bytes: 34_359_738_368,
                system_available_bytes: 17_179_869_184,
            },
            cpu: CpuStatus {
                process_percent: 2.3,
                system_percent: 41.0,
                logical_cores: 16,
            },
            storage: StorageStatus {
                kind: "sled".to_string(),
                used_bytes: Some(1_048_576),
            },
            processes: ProcessCounts {
                total: 7,
                active: 2,
            },
            health: HealthStatus {
                state: "degraded".to_string(),
                loops: vec![
                    LoopStatus {
                        name: "trigger".to_string(),
                        state: "healthy".to_string(),
                        consecutive_failures: 0,
                        backoff_ticks: 0,
                    },
                    LoopStatus {
                        name: "retry".to_string(),
                        state: "degraded".to_string(),
                        consecutive_failures: 5,
                        backoff_ticks: 3,
                    },
                ],
            },
            config: json!({"tick_interval_secs": 15}),
        }
    }

    /// The render is the command's product: every section the server answers
    /// with gets a line, degraded loops are named, and the config is dumped
    /// as the json it arrived as.
    #[test]
    fn the_answer_renders_every_section() {
        let text = render(&sample());
        assert!(text.contains("version      0.25.0"), "{text}");
        assert!(
            text.contains("health       degraded (degraded loops: retry)"),
            "{text}"
        );
        assert!(text.contains("processes    total 7, active 2"), "{text}");
        assert!(
            text.contains("memory       rss 15.0 MB (15728640 B)"),
            "{text}"
        );
        assert!(
            text.contains("cpu          process 2.3%, system 41.0% (16 cores)"),
            "{text}"
        );
        assert!(
            text.contains("storage      sled, used 1.0 MB (1048576 B)"),
            "{text}"
        );
        assert!(
            text.contains("  loop       retry      degraded failures 5 backoff 3 ticks"),
            "{text}"
        );
        assert!(
            text.contains("  loop       trigger    healthy  failures 0 backoff 0 ticks"),
            "{text}"
        );
        assert!(text.contains("\"tick_interval_secs\": 15"), "{text}");
    }

    /// A backend that cannot size itself says so instead of printing a
    /// fabricated number.
    #[test]
    fn an_unknown_storage_size_is_said() {
        let mut status = sample();
        status.storage.used_bytes = None;
        let text = render(&status);
        assert!(text.contains("storage      sled, size unknown"), "{text}");
    }

    /// Small sizes stay in bytes, not fractions of a KB.
    #[test]
    fn small_sizes_stay_in_bytes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(0), "0 B");
    }
}
