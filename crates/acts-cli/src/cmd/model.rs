use super::CommandRunner as Command;
use crate::{client, util};
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

pub async fn process(parent: &mut Command<'_>, command: &ModelCommands) -> anyhow::Result<()> {
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

pub async fn deploy(parent: &mut Command<'_>, path: &PathBuf) -> anyhow::Result<String> {
    let mut ret = String::new();
    let text = std::fs::read_to_string(path).map_err(|err| {
        anyhow::anyhow!("failed to read the model file {}: {err}", path.display())
    })?;
    let resp = parent.deploy(&text).await?;
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
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let query = util::to_query(offset, count, query_by, order_by);
    let resp = parent
        .send::<PageData<ModelInfo>>("model:ls", Vars::new().with("query", query))
        .await?;
    let data = client::payload("model:ls", resp.data.as_ref())?;
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
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

pub async fn get(
    parent: &mut Command<'_>,
    id: &str,
    fmt: &Option<String>,
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("id", id);
    if let Some(fmt) = fmt {
        options.set("fmt", fmt);
    };

    let resp = parent.send::<ModelInfo>("model:get", options).await?;
    let model = client::payload("model:get", resp.data)?;
    ret.push_str(&model.data);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn rm(parent: &mut Command<'_>, id: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let resp = parent
        .send::<bool>("model:rm", Vars::new().with("id", id))
        .await?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
