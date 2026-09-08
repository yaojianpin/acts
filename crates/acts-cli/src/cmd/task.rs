use super::CommandRunner as Command;
use crate::util;
use acts_channel::{
    Vars,
    model::{Expr, OrderBy, PageData, TaskInfo},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommands,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommands {
    #[command(about = "get task by id")]
    Get {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
    },
    #[command(about = "list all tasks")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q state=running -Q type=irq", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O state -O type!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &TaskCommands) -> Result<(), String> {
    let ret = match command {
        TaskCommands::Get { pid, tid } => get(parent, pid, tid).await,
        TaskCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
    }?;

    parent.output(&ret);
    Ok(())
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
        .send::<PageData<TaskInfo>>("task:ls", Vars::new().with("query", query))
        .await
        .map_err(|err| err.message().to_string())?;

    let data = resp.data.as_ref().unwrap();
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "type",
        "pid",
        "tid",
        "name",
        "nid",
        "state",
        "start time",
        "end time",
    ]);
    for p in &data.rows {
        table.add_row(vec![
            p.r#type.to_string(),
            p.pid.to_string(),
            p.id.to_string(),
            p.name.to_string(),
            p.nid.to_string(),
            p.state.to_string(),
            util::local_time(p.start_time),
            util::local_time(p.end_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}

pub async fn get(parent: &mut Command<'_>, pid: &str, tid: &str) -> Result<String, String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("pid", pid);
    options.set("tid", tid);
    let resp = parent
        .client
        .send::<TaskInfo>("task:get", options)
        .await
        .map_err(|err| err.message().to_string())?;
    let task = resp.data.unwrap();
    ret.push_str(&serde_json::to_string_pretty(&task).unwrap());
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
