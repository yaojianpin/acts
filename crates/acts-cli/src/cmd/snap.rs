use super::CommandRunner as Command;
use crate::{client, util};
use acts_channel::Vars;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub command: SnapshotCommands,
}

#[derive(Debug, Subcommand)]
pub enum SnapshotCommands {
    #[command(about = "update or insert a snapshot value on the server")]
    Upsert {
        #[arg(help = "snapshot target name, e.g. profile")]
        name: String,
        #[arg(short, long, default_value = "", help = "scope key of the snapshot")]
        scope: String,
        #[arg(long, help = "monotonic revision of the value")]
        rev: u64,
        #[arg(short, long, help = "snapshot data (json object)", value_parser = util::parse_json)]
        data: serde_json::Value,
    },
    #[command(about = "remove a snapshot value on the server (tombstone)")]
    Remove {
        #[arg(help = "snapshot target name, e.g. profile")]
        name: String,
        #[arg(short, long, default_value = "", help = "scope key of the snapshot")]
        scope: String,
    },
    #[command(about = "query one snapshot value by name and scope")]
    Get {
        #[arg(help = "snapshot target name, e.g. profile")]
        name: String,
        #[arg(short, long, default_value = "", help = "scope key of the snapshot")]
        scope: String,
    },
    #[command(about = "query every scope of one snapshot target")]
    Ls {
        #[arg(help = "snapshot target name, e.g. profile")]
        name: String,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &SnapshotCommands) -> anyhow::Result<()> {
    let ret = match command {
        SnapshotCommands::Upsert {
            name,
            scope,
            rev,
            data,
        } => upsert(parent, name, scope, *rev, data).await,
        SnapshotCommands::Remove { name, scope } => remove(parent, name, scope).await,
        SnapshotCommands::Get { name, scope } => get(parent, name, scope).await,
        SnapshotCommands::Ls { name } => ls(parent, name).await,
    }?;

    parent.output(&ret);
    Ok(())
}

pub async fn upsert(
    parent: &mut Command<'_>,
    name: &str,
    scope: &str,
    rev: u64,
    data: &serde_json::Value,
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let mut vars = Vars::new()
        .with("name", name)
        .with("scope", scope)
        .with("rev", rev);
    vars.insert("data".to_string(), data.clone());
    let resp = parent.send::<bool>("snap:upsert", vars).await?;

    if client::payload("snap:upsert", resp.data)? {
        ret.push_str(&format!(
            "snapshot '{name}' scope '{scope}' updated (rev {rev})"
        ));
    } else {
        ret.push_str("snapshot update returned false");
    }
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn remove(parent: &mut Command<'_>, name: &str, scope: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let vars = Vars::new().with("name", name).with("scope", scope);
    let resp = parent.send::<bool>("snap:remove", vars).await?;

    if client::payload("snap:remove", resp.data)? {
        ret.push_str(&format!("snapshot '{name}' scope '{scope}' removed"));
    } else {
        ret.push_str("snapshot remove returned false");
    }
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
pub async fn get(parent: &mut Command<'_>, name: &str, scope: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let vars = Vars::new().with("name", name).with("scope", scope);
    let resp = parent.send::<serde_json::Value>("snap:get", vars).await?;

    match resp.data {
        Some(value) if !value.is_null() => {
            let text = util::to_yaml(&value)?;
            ret.push_str(&text);
        }
        _ => ret.push_str("snapshot not found"),
    }
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn ls(parent: &mut Command<'_>, name: &str) -> anyhow::Result<String> {
    use comfy_table::Table;

    let mut ret = String::new();
    let resp = parent
        .send::<serde_json::Value>("snap:ls", Vars::new().with("name", name))
        .await?;

    let rows = client::payload("snap:ls", resp.data)?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        ret.push_str(&format!("no snapshot data for target '{name}'"));
        return Ok(ret);
    }
    let mut table = Table::new();
    table.set_header(vec!["scope", "rev", "timestamp", "data"]);
    for row in &rows {
        table.add_row(vec![
            row["scope"].as_str().unwrap_or_default().to_string(),
            row["rev"].as_u64().unwrap_or_default().to_string(),
            row["timestamp"].as_i64().unwrap_or_default().to_string(),
            serde_json::to_string(&row["data"]).unwrap_or_default(),
        ]);
    }
    println!("{table}");
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
