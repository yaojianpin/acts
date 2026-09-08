use super::CommandRunner as Command;
use crate::util;
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

pub async fn process(parent: &mut Command<'_>, command: &SnapshotCommands) -> Result<(), String> {
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
) -> Result<String, String> {
    let mut ret = String::new();
    let mut vars = Vars::new()
        .with("name", name)
        .with("scope", scope)
        .with("rev", rev);
    vars.insert("data".to_string(), data.clone());
    let resp = parent
        .client
        .send::<bool>("snap:upsert", vars)
        .await
        .map_err(|err| err.message().to_string())?;

    if resp.data.unwrap_or_default() {
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

pub async fn remove(parent: &mut Command<'_>, name: &str, scope: &str) -> Result<String, String> {
    let mut ret = String::new();
    let vars = Vars::new().with("name", name).with("scope", scope);
    let resp = parent
        .client
        .send::<bool>("snap:remove", vars)
        .await
        .map_err(|err| err.message().to_string())?;

    if resp.data.unwrap_or_default() {
        ret.push_str(&format!("snapshot '{name}' scope '{scope}' removed"));
    } else {
        ret.push_str("snapshot remove returned false");
    }
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
pub async fn get(parent: &mut Command<'_>, name: &str, scope: &str) -> Result<String, String> {
    let mut ret = String::new();
    let vars = Vars::new().with("name", name).with("scope", scope);
    let resp = parent
        .client
        .send::<serde_json::Value>("snap:get", vars)
        .await
        .map_err(|err| err.message().to_string())?;

    match resp.data {
        Some(value) if !value.is_null() => {
            let text = serde_yaml::to_string(&value).map_err(|err| err.to_string())?;
            ret.push_str(&text);
        }
        _ => ret.push_str("snapshot not found"),
    }
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn ls(parent: &mut Command<'_>, name: &str) -> Result<String, String> {
    use comfy_table::Table;

    let mut ret = String::new();
    let resp = parent
        .client
        .send::<serde_json::Value>("snap:ls", Vars::new().with("name", name))
        .await
        .map_err(|err| err.message().to_string())?;

    let rows = resp.data.unwrap_or_default();
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
