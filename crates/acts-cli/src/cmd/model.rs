use super::CommandRunner as Command;
use crate::util;
use acts_channel::{
    Vars,
    model::{Expr, ModelInfo, OrderBy, PageData},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use std::path::PathBuf;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct ModelArgs {
    #[command(subcommand)]
    pub command: ModelCommands,
}

#[derive(Debug, Subcommand)]
pub enum ModelCommands {
    #[command(about = "get model by id")]
    Get {
        #[arg(help = "model id")]
        id: String,

        #[arg(short, long, help = "format to print, the value should be one of json and tree", value_parser(["json", "tree"]))]
        fmt: Option<String>,
    },

    #[command(about = "list all models")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q name=approve", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O create_time -O update_time!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },

    #[command(about = "remove a model by id")]
    Rm {
        #[arg(help = "model id")]
        id: String,
    },

    #[command(about = "deploy a workflow model")]
    Deploy {
        #[arg(required = true, help = "model file path")]
        path: PathBuf,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &ModelCommands) -> Result<(), String> {
    let ret = match command {
        ModelCommands::Get { id, fmt } => get(parent, id, fmt).await,
        ModelCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        ModelCommands::Rm { id } => rm(parent, id).await,
        ModelCommands::Deploy { path } => deploy(parent, path).await,
    }?;

    parent.output(&ret);
    Ok(())
}

async fn deploy(parent: &mut Command<'_>, path: &PathBuf) -> Result<String, String> {
    let mut ret = String::new();
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let resp = parent
        .client
        .deploy(&text, None)
        .await
        .map_err(|err| err.message().to_string())?;
    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

async fn ls(
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
        .send::<PageData<ModelInfo>>("model:ls", Vars::new().with("query", query))
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
        "version",
        "size",
        "create time",
        "update time",
    ]);
    for m in &data.rows {
        table.add_row(vec![
            m.id.to_string(),
            m.name.to_string(),
            m.ver.to_string(),
            util::size(m.size),
            util::local_time(m.create_time),
            util::local_time(m.update_time),
        ]);
    }

    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}

async fn get(parent: &mut Command<'_>, id: &str, fmt: &Option<String>) -> Result<String, String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("id", id);
    if let Some(fmt) = fmt {
        options.set("fmt", fmt);
    };

    let resp = parent
        .client
        .send::<ModelInfo>("model:get", options)
        .await
        .map_err(|err| err.message().to_string())?;
    let model = resp.data.unwrap();
    ret.push_str(&model.data);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

async fn rm(parent: &mut Command<'_>, id: &str) -> Result<String, String> {
    let mut ret = String::new();
    let resp = parent
        .client
        .send::<bool>("model:rm", Vars::new().with("id", id))
        .await
        .map_err(|err| err.message().to_string())?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
