//! `auth` — log in, log out, and manage the engine's users.
//!
//! The CLI holds one connection: `auth login` swaps the credential it carries
//! (and stores the session so the next run reuses it), `auth logout` drops
//! it. `auth user …` administers the engine's user registry (`acl:setuser`,
//! `acl:getuser`, `acl:users`, `acl:deluser`) and therefore needs an
//! administrator: an ordinary user is refused by the server, not here.

use super::CommandRunner as Command;
use crate::{client, session, util};
use acts_channel::Vars;
use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommands,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommands {
    #[command(about = "log in as a user and store the session")]
    Login {
        #[arg(help = "user name")]
        user: String,
        #[arg(
            short,
            long,
            env = "ACTS_PASSWORD",
            hide_env_values = true,
            help = "password; prompted for when omitted (ACTS_PASSWORD is preferred over the argument)"
        )]
        password: Option<String>,
    },
    #[command(about = "log out and forget the stored session")]
    Logout,
    #[command(about = "show the identity the server resolved for this session")]
    Whoami,
    #[command(about = "manage the engine's users")]
    User(UserArgs),
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommands,
}

#[derive(Debug, Subcommand)]
pub enum UserCommands {
    #[command(about = "get one user's policy")]
    Get {
        #[arg(help = "user name")]
        user: String,
    },
    #[command(about = "list user names")]
    Ls,
    #[command(about = "create or update a user (passwords and grants)")]
    Set {
        #[arg(help = "user name")]
        user: String,
        #[arg(
            long,
            value_name = "PATTERN",
            help = "command pattern or @catalog group to allow (repeatable; replaces the list). \
                    groups: @read, @write, @deploy, @execute, @all. \
                    example: --allow 'model:*' --allow @read"
        )]
        allow: Vec<String>,
        #[arg(
            long,
            value_name = "PATTERN",
            help = "command pattern or @catalog group to deny (repeatable; wins over --allow)"
        )]
        deny: Vec<String>,
        #[arg(
            long = "pattern",
            value_name = "RN",
            help = "resource name pattern the user may deploy and run (repeatable). \
                    example: --pattern 'orders:*'"
        )]
        patterns: Vec<String>,
        #[arg(
            long = "snapshot",
            value_name = "TARGET=GLOB[,GLOB]",
            value_parser = util::parse_snapshot,
            help = "snapshot scopes the user owns, per target; $subject is the user name. \
                    example: --snapshot 'secrets=$subject'"
        )]
        snapshot: Vec<(String, Vec<String>)>,
        #[arg(long, value_name = "PASSWORD", help = "add a password (repeatable)")]
        password: Vec<String>,
        #[arg(
            long = "rm-password",
            value_name = "PASSWORD",
            help = "remove a password by its plaintext (repeatable)"
        )]
        rm_password: Vec<String>,
        #[arg(long, help = "disable the user (login is refused, live sessions die)")]
        disable: bool,
        #[arg(long, help = "(re)enable the user")]
        enable: bool,
    },
    #[command(about = "delete a user (the builtin admin cannot be deleted)")]
    Rm {
        #[arg(help = "user name")]
        user: String,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &AuthCommands) -> anyhow::Result<()> {
    match command {
        AuthCommands::Login { user, password } => login(parent, user, password.as_deref()).await,
        AuthCommands::Logout => logout(parent).await,
        AuthCommands::Whoami => whoami(parent).await,
        AuthCommands::User(args) => match &args.command {
            UserCommands::Get { user } => get(parent, user).await,
            UserCommands::Ls => ls(parent).await,
            UserCommands::Set {
                user,
                allow,
                deny,
                patterns,
                snapshot,
                password,
                rm_password,
                disable,
                enable,
            } => {
                set(
                    parent,
                    user,
                    allow,
                    deny,
                    patterns,
                    snapshot,
                    password,
                    rm_password,
                    *disable,
                    *enable,
                )
                .await
            }
            UserCommands::Rm { user } => rm(parent, user).await,
        },
    }?;

    Ok(())
}

/// Log in and store the session so the next run reuses it.
pub async fn login(
    parent: &mut Command<'_>,
    user: &str,
    password: Option<&str>,
) -> anyhow::Result<()> {
    let password = match password {
        Some(password) => password.to_string(),
        None => util::prompt_password()?,
    };
    let url = parent.client().url().to_string();
    let tokens = parent
        .client_mut()
        .login(user, &password)
        .await
        .map_err(|err| client::action_failed("acl:login", err))?;
    session::save(&url, user, &tokens)?;
    parent.output(&format!(
        "logged in as '{user}' (access token valid for {}s; session stored for {url})",
        tokens
            .expires_at
            .saturating_sub(chrono::Utc::now().timestamp_millis())
            / 1000
    ));
    Ok(())
}

pub async fn logout(parent: &mut Command<'_>) -> anyhow::Result<()> {
    let url = parent.client().url().to_string();
    let done = parent
        .client_mut()
        .logout()
        .await
        .map_err(|err| client::action_failed("acl:logout", err))?;
    session::clear(&url)?;
    parent.output(if done {
        "logged out"
    } else {
        "no session to log out of"
    });
    Ok(())
}

pub async fn whoami(parent: &mut Command<'_>) -> anyhow::Result<()> {
    let ret = parent
        .send::<serde_json::Value>("acl:whoami", Vars::new())
        .await?;
    let who = client::payload("acl:whoami", ret.data)?;
    parent.output(&util::to_json(&who)?);
    Ok(())
}

pub async fn get(parent: &mut Command<'_>, user: &str) -> anyhow::Result<()> {
    let ret = parent
        .send::<serde_json::Value>("acl:getuser", Vars::new().with("user", user))
        .await?;
    let value = client::payload("acl:getuser", ret.data)?;
    parent.output(&util::to_json(&value)?);
    Ok(())
}

pub async fn ls(parent: &mut Command<'_>) -> anyhow::Result<()> {
    let ret = parent
        .send::<serde_json::Value>("acl:users", Vars::new())
        .await?;
    let value = client::payload("acl:users", ret.data)?;
    parent.output(&util::to_json(&value)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn set(
    parent: &mut Command<'_>,
    user: &str,
    allow: &[String],
    deny: &[String],
    patterns: &[String],
    snapshot: &[(String, Vec<String>)],
    password: &[String],
    rm_password: &[String],
    disable: bool,
    enable: bool,
) -> anyhow::Result<()> {
    let mut spec = serde_json::Map::new();
    spec.insert("name".to_string(), json!(user));
    // only what the user asked to change travels: a list left off keeps its
    // current value, which is what makes `set` an edit rather than a replace
    if !allow.is_empty() {
        spec.insert("allow".to_string(), json!(allow));
    }
    if !deny.is_empty() {
        spec.insert("deny".to_string(), json!(deny));
    }
    if !patterns.is_empty() {
        spec.insert("patterns".to_string(), json!(patterns));
    }
    if !snapshot.is_empty() {
        let scopes: serde_json::Map<String, serde_json::Value> = snapshot
            .iter()
            .map(|(target, modes)| (target.clone(), json!(modes)))
            .collect();
        spec.insert("snapshot".to_string(), json!(scopes));
    }
    if !password.is_empty() {
        spec.insert("add_passwords".to_string(), json!(password));
    }
    if !rm_password.is_empty() {
        spec.insert("rm_passwords".to_string(), json!(rm_password));
    }
    if disable && enable {
        anyhow::bail!("--disable and --enable are mutually exclusive");
    }
    if disable {
        spec.insert("enabled".to_string(), json!(false));
    }
    if enable {
        spec.insert("enabled".to_string(), json!(true));
    }

    let mut vars = Vars::new();
    vars.set("user", serde_json::Value::Object(spec));
    let ret = parent.send::<bool>("acl:setuser", vars).await?;
    let _ = client::payload("acl:setuser", ret.data)?;
    parent.output(&format!("user '{user}' saved"));
    Ok(())
}

pub async fn rm(parent: &mut Command<'_>, user: &str) -> anyhow::Result<()> {
    let ret = parent
        .send::<bool>("acl:deluser", Vars::new().with("user", user))
        .await?;
    let _ = client::payload("acl:deluser", ret.data)?;
    parent.output(&format!("user '{user}' deleted"));
    Ok(())
}
