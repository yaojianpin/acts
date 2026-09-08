use super::CommandRunner as Command;
use crate::util;
use acts_channel::{
    Vars,
    model::{EventInfo, Expr, OrderBy, PageData},
};
use clap::{Args, Subcommand};
use comfy_table::Table;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct EventArgs {
    #[command(subcommand)]
    pub command: EventCommands,
}

#[derive(Debug, Subcommand)]
pub enum EventCommands {
    #[command(about = "get event by id")]
    Get {
        #[arg(help = "event id")]
        id: String,
    },
    #[command(about = "list all events")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q id=123", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O start_time -O update_time!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
    #[command(about = "start a event")]
    Start {
        #[arg(help = "event id")]
        id: String,
        #[arg(short, long, help = "event params", value_parser = util::parse_json)]
        params: Option<serde_json::Value>,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &EventCommands) -> Result<(), String> {
    let ret = match command {
        EventCommands::Get { id } => get(parent, id).await,
        EventCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        EventCommands::Start { id, params } => start(parent, id, params).await,
    }?;

    parent.output(&ret);
    Ok(())
}

pub async fn start(
    parent: &mut Command<'_>,
    id: &str,
    params: &Option<serde_json::Value>,
) -> Result<String, String> {
    let mut ret = String::new();
    let mut vars = Vars::new().with("id", id);
    if let Some(param) = params {
        vars.insert("params".to_string(), param.clone());
    }
    let resp = parent
        .client
        .send::<Option<Vars>>("evt:start", vars)
        .await
        .map_err(|err| err.message().to_string())?;
    ret.push_str(&format!("{:?}", resp.data.unwrap()));

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
        .send::<EventInfo>("evt:get", options)
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
        .send::<PageData<EventInfo>>("evt:ls", Vars::new().with("query", query))
        .await
        .map_err(|err| err.message().to_string())?;

    let data = resp.data.as_ref().unwrap();
    let mut table = Table::new();
    table.set_header(vec!["id", "name", "mid", "ver", "uses", "create time"]);
    for p in &data.rows {
        table.add_row(vec![
            p.id.to_string(),
            p.name.to_string(),
            p.mid.to_string(),
            p.ver.to_string(),
            p.uses.to_string(),
            util::local_time(p.create_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}
