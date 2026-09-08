use super::CommandRunner as Command;
use crate::util;
use acts_channel::{
    Vars,
    model::{Expr, OrderBy, PageData, ProcInfo},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct ProcArgs {
    #[command(subcommand)]
    pub command: ProcCommands,
}

#[derive(Debug, Subcommand)]
pub enum ProcCommands {
    #[command(about = "get proc by id")]
    Get {
        #[arg(help = "proc id")]
        id: String,
        #[arg(short, long, help = "format to print, the value should be one of json and tree", value_parser(["json", "tree"]))]
        fmt: Option<String>,
    },
    #[command(about = "list all procs")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q mid=approve", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O mid -O start_time -O end_time!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
    #[command(about = "deploy a workflow model")]
    Start {
        #[arg(required = true, help = "proc id")]
        id: String,
        #[arg(short, long, help = "specify a pid for proc")]
        pid: Option<String>,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &ProcCommands) -> Result<(), String> {
    let ret = match command {
        ProcCommands::Get { id, fmt } => get(parent, id, fmt).await,
        ProcCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        ProcCommands::Start { id, pid } => start(parent, id, pid, &parent.vars.clone()).await,
    }?;

    parent.output(&ret);
    Ok(())
}

async fn start(
    parent: &mut Command<'_>,
    mid: &str,
    pid: &Option<String>,
    vars: &Vars,
) -> Result<String, String> {
    let mut ret = String::new();
    let mut options = Vars::new().extend(vars);
    if let Some(pid) = pid {
        options.set("pid", pid);
    }
    let resp = parent
        .client
        .start(mid, options)
        .await
        .map_err(|err| err.message().to_string())?;

    let pid = resp.data.unwrap();
    ret.push_str(&format!("pid={pid}"));
    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn get(
    parent: &mut Command<'_>,
    pid: &str,
    fmt: &Option<String>,
) -> Result<String, String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("pid", pid);

    if let Some(fmt) = fmt {
        options.set("fmt", fmt);
    };

    let resp = parent
        .client
        .send::<ProcInfo>("proc:get", options)
        .await
        .map_err(|err| err.message().to_string())?;

    let proc = resp.data.unwrap();
    ret.push_str(&serde_json::to_string_pretty(&proc).unwrap());
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
        .send::<PageData<ProcInfo>>("proc:ls", Vars::new().with("query", query))
        .await
        .map_err(|err| err.message().to_string())?;
    let data = resp.data.as_ref().unwrap();
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["pid", "name", "model id", "state", "start time"]);
    for p in &data.rows {
        table.add_row(vec![
            p.id.to_string(),
            p.name.to_string(),
            p.mid.to_string(),
            p.state.to_string(),
            util::local_time(p.start_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}
