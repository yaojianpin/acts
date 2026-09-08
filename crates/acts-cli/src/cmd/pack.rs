use super::CommandRunner as Command;
use crate::util;
use acts_channel::{
    Vars,
    model::{Expr, OrderBy, Package, PackageInfo, PageData},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use std::path::PathBuf;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct PacakgeArgs {
    #[command(subcommand)]
    pub command: PacakgeCommands,
}

#[derive(Debug, Subcommand)]
pub enum PacakgeCommands {
    #[command(
        about = "publish a package",
        long_about = r#"publish a package with yml file
Example: publish ./package.yml
// package.yml
id: package id, such as acts.core.irq
name: package name
desc: test desc
icon: icon-test
doc: help url
version: 0.1.0
schema: json schema for package params
run_as: irq or msg
resources: array json for operations
catalog: package catalog
"#
    )]
    Publish {
        #[arg(required = true, help = "package file path")]
        path: PathBuf,
    },
    #[command(about = "get package by id")]
    Get {
        #[arg(help = "package id")]
        id: String,
    },
    #[command(about = "list all packages")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q id=123 -Q name~a\n =: equal\n~: match", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys, '!' means order by desc. \nexample: -O start_time -O update_time!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
    #[command(about = "remove a package by id")]
    Rm {
        #[arg(help = "package id")]
        id: String,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &PacakgeCommands) -> Result<(), String> {
    let ret = match command {
        PacakgeCommands::Get { id } => get(parent, id).await,
        PacakgeCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        PacakgeCommands::Rm { id } => rm(parent, id).await,
        PacakgeCommands::Publish { path } => publish(parent, path).await,
    }?;

    parent.output(&ret);
    Ok(())
}

pub async fn publish(parent: &mut Command<'_>, path: &PathBuf) -> Result<String, String> {
    let mut ret = String::new();
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;

    let package = serde_yaml::from_str::<Package>(&text).map_err(|err| err.to_string())?;
    let resp = parent
        .client
        .publish(&package)
        .await
        .map_err(|err| err.message().to_string())?;
    ret.push_str(&format!("{}", resp.data.unwrap()));

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn get(parent: &mut Command<'_>, id: &str) -> Result<String, String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("id", id);
    let resp = parent
        .client
        .send::<PackageInfo>("pack:get", options)
        .await
        .map_err(|err| err.message().to_string())?;

    let package = resp.data.unwrap();

    let text = serde_yaml::to_string(&package).map_err(|err| err.to_string())?;
    ret.push_str(&text);

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn ls(
    parent: &mut Command<'_>,
    offset: &Option<u32>,
    count: &Option<u32>,
    query_by: &Vec<Expr>,
    order_by: &Vec<OrderBy>,
) -> Result<String, String> {
    let mut ret = String::new();
    let query = util::to_query(offset, count, query_by, order_by);
    let resp = parent
        .client
        .send::<PageData<PackageInfo>>("pack:ls", Vars::new().with("query", query))
        .await
        .map_err(|err| err.message().to_string())?;

    let data = resp.data.as_ref().unwrap();
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "id",
        "name",
        "desc",
        "run_as",
        "catalog",
        "version",
        "create time",
    ]);
    for p in &data.rows {
        table.add_row(vec![
            p.id.to_string(),
            p.name.to_string(),
            p.desc.to_string(),
            p.run_as.to_string(),
            p.catalog.to_string(),
            p.version.to_string(),
            util::local_time(p.create_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}

pub async fn rm(parent: &mut Command<'_>, id: &str) -> Result<String, String> {
    let mut ret = String::new();
    let resp = parent
        .client
        .send::<bool>("pack:rm", Vars::new().with("id", id))
        .await
        .map_err(|err| err.message().to_string())?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
