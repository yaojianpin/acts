use super::CommandRunner as Command;
use crate::{client, util};
use acts_channel::{
    ActsOptions, Vars,
    model::{Expr, MessageInfo, OrderBy, PageData},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct MessageArgs {
    #[command(subcommand)]
    pub command: MessageCommands,
}

#[derive(Debug, Subcommand)]
pub enum MessageCommands {
    #[command(about = "get message by id")]
    Get {
        #[arg(help = "message id")]
        id: String,
    },
    #[command(about = "ack message by id")]
    Ack {
        #[arg(help = "message id")]
        id: String,
    },
    #[command(about = "list all messages")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the item count")]
        count: Option<u32>,

        #[arg(short='Q', long, help = "query by keys. \nexample: -Q state=running -Q type=irq", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,

        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O state -O key!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
    #[command(about = "remove a message by id")]
    Rm {
        #[arg(help = "message id")]
        id: String,
    },
    #[command(about = "clear all error messages")]
    Clear {
        #[arg(short, long, help = "proc id")]
        pid: Option<String>,
    },

    #[command(about = "redsend stored messages caused by error")]
    Redo,

    #[command(about = "subscribe server messages")]
    Sub {
        #[arg(help = "client id")]
        client_id: String,
        #[arg(
            short,
            long,
            help = "message type in glob pattern, the type includes workflow, step, branch and act"
        )]
        r#type: Option<String>,
        #[arg(
            short,
            long,
            help = "message type in glob pattern, the state includes created, completed, error, cancelled, aborted, skipped and backed"
        )]
        state: Option<String>,

        #[arg(short='O', long, help = "custom message options in glob pattern. \nexample: -O tag=mytag -O tag2=mytag2", value_parser = util::parse_options)]
        options: Vec<(String, String)>,

        #[arg(short, long, help = "message uses in glob pattern")]
        uses: Option<String>,
        #[arg(
            short,
            long,
            default_value_t = true,
            help = "auto ack message by client, if false you should ack message from you app"
        )]
        ack: bool,
    },
    #[command(about = "unsubscribe server messages by client id")]
    Unsub {
        #[arg(help = "client id")]
        client_id: String,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &MessageCommands) -> anyhow::Result<()> {
    let ret = match command {
        MessageCommands::Get { id } => get(parent, id).await,
        MessageCommands::Ack { id } => ack(parent, id).await,
        MessageCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        MessageCommands::Rm { id } => rm(parent, id).await,
        MessageCommands::Clear { pid } => clear(parent, pid).await,
        MessageCommands::Redo => redo(parent).await,
        MessageCommands::Sub {
            client_id,
            r#type,
            state,
            options,
            uses,
            ack,
        } => sub(parent, client_id, r#type, state, uses, ack, options).await,
        MessageCommands::Unsub { client_id } => unsub(parent, client_id).await,
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
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let query = util::to_query(offset, count, query_by, order_by);
    let resp = parent
        .send::<PageData<MessageInfo>>("msg:ls", Vars::new().with("query", query))
        .await?;

    let data = client::payload("msg:ls", resp.data.as_ref())?;
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "type",
        "id",
        "tid",
        "state",
        "uses",
        "retries",
        "status",
        "create time",
        "update time",
    ]);
    for p in data.rows.iter() {
        table.add_row(vec![
            p.r#type.to_string(),
            p.id.to_string(),
            p.tid.to_string(),
            p.state.to_string(),
            p.uses.clone().unwrap_or_default(),
            p.retry_times.to_string(),
            p.status.to_string(),
            util::local_time(p.create_time),
            util::local_time(p.update_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}

pub async fn get(parent: &mut Command<'_>, id: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("id", id);
    let resp = parent.send::<MessageInfo>("msg:get", options).await?;
    let message = client::payload("msg:get", resp.data)?;
    ret.push_str(&util::to_json(&message)?);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn ack(parent: &mut Command<'_>, id: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let resp = parent.ack(id).await?;

    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn redo(parent: &mut Command<'_>) -> anyhow::Result<String> {
    let mut ret = String::new();
    let options = Vars::new();
    let resp = parent.send::<()>("msg:redo", options).await?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn rm(parent: &mut Command<'_>, id: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let resp = parent
        .send::<bool>("msg:rm", Vars::new().with("id", id))
        .await?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn clear(parent: &mut Command<'_>, pid: &Option<String>) -> anyhow::Result<String> {
    let mut ret = String::new();
    let resp = parent
        .send::<()>("msg:clear", Vars::new().with("pid", pid))
        .await?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

#[allow(clippy::too_many_arguments)]
async fn sub(
    parent: &mut Command<'_>,
    client_id: &str,
    r#type: &Option<String>,
    state: &Option<String>,
    uses: &Option<String>,
    ack: &bool,
    options: &[(String, String)],
) -> anyhow::Result<String> {
    let default_value = "*".to_string();
    // * means to sub all messages
    let r#type = r#type.as_ref().unwrap_or(&default_value);
    let state = state.as_ref().unwrap_or(&default_value);
    let uses = uses.as_ref().unwrap_or(&default_value);
    let sub = parent
        .client
        .subscribe(
            client_id,
            |m| match serde_json::to_string(&m) {
                Ok(text) => println!("[message]: {text}"),
                Err(err) => eprintln!("[subscription] message print failed: {err}"),
            },
            |err| eprintln!("[subscription] {err}"),
            &ActsOptions {
                r#type: Some(r#type.to_string()),
                state: Some(state.to_string()),
                options: options.iter().cloned().collect(),
                uses: Some(uses.to_string()),
                ack: Some(*ack),
            },
        )
        .await
        .map_err(|err| client::action_failed("msg:sub", err))?;

    // the feed runs in the background: its end must stay visible
    tokio::spawn(async move {
        match sub.wait().await {
            Ok(()) => eprintln!("[subscription] closed by the server"),
            Err(err) => eprintln!("[subscription] closed: {err}"),
        }
    });

    Ok(format!("subscribed server messages as '{client_id}'"))
}

pub async fn unsub(parent: &mut Command<'_>, client_id: &str) -> anyhow::Result<String> {
    let mut ret = String::new();
    let resp = parent
        .send::<()>("msg:unsub", Vars::new().with("client_id", client_id))
        .await?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
