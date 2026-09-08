use crate::util;

use super::CommandRunner as Command;
use acts_channel::Vars;
use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct ActArgs {
    #[command(subcommand)]
    pub command: ActCommands,
}

#[derive(Debug, Subcommand)]
pub enum ActCommands {
    #[command(about = "submit a running act")]
    Submit {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "complete a running act")]
    Complete {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "skip a running act")]
    Skip {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "abort a running act")]
    Abort {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "set a running act as error")]
    Error {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(help = "error code")]
        ecode: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "cancel a completed act")]
    Cancel {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "back a running act to a historical step")]
    Back {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(required = true, help = "model file path")]
        to: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "push a new act under a step")]
    Push {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },

    #[command(about = "remove an act from a step")]
    Remove {
        #[arg(help = "proc id")]
        pid: String,
        #[arg(help = "task id")]
        tid: String,
        #[arg(short, long, help="vars in K=V format\nthe V can be number, string, or json, \nif the V contains whitesapce, please wrap it in `'` or `\"`\nexample: \n-v a=1 -v b=abc -v c='[2, 3, 4]' -v d='{ \"value\": 100 }' -v e=null", value_parser = util::parse_key_json::<String>)]
        vars: Vec<(String, serde_json::Value)>,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &ActCommands) -> Result<(), String> {
    let ret = match command {
        ActCommands::Submit { pid, tid, vars } => send(parent, "act:submit", pid, tid, vars).await,
        ActCommands::Complete { pid, tid, vars } => {
            send(parent, "act:complete", pid, tid, vars).await
        }
        ActCommands::Skip { pid, tid, vars } => send(parent, "act:skip", pid, tid, vars).await,
        ActCommands::Abort { pid, tid, vars } => send(parent, "act:abort", pid, tid, vars).await,
        ActCommands::Error {
            pid,
            tid,
            ecode,
            vars,
        } => {
            let mut vars = vars.clone();
            vars.push(("ecode".to_string(), json!(ecode)));
            let ret = send(parent, "act:error", pid, tid, &vars).await?;
            Ok(ret)
        }
        ActCommands::Cancel { pid, tid, vars } => send(parent, "act:cancel", pid, tid, vars).await,
        ActCommands::Back { pid, tid, to, vars } => {
            let mut vars = vars.clone();
            vars.push(("to".to_string(), json!(to)));
            send(parent, "act:back", pid, tid, &vars).await
        }
        ActCommands::Push { pid, tid, vars } => send(parent, "push", pid, tid, vars).await,
        ActCommands::Remove { pid, tid, vars } => send(parent, "act:remove", pid, tid, vars).await,
    }?;

    parent.output(&ret);
    Ok(())
}

async fn send(
    parent: &mut Command<'_>,
    name: &str,
    pid: &str,
    tid: &str,
    vars: &Vec<(String, serde_json::Value)>,
) -> Result<String, String> {
    let mut ret = String::new();

    #[allow(unused_mut, unused_variables)]
    let mut options = Vars::new().with("pid", pid).with("tid", tid);
    for (k, v) in vars {
        options.set(k, v);
    }
    let resp = parent
        .client
        .send::<()>(name, options)
        .await
        .map_err(|err| err.message().to_string())?;

    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}
