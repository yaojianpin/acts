use crate::{
    sch::Context,
    utils::{self, consts},
    ActError, ActFor, Result, TaskState, Vars,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize, PartialEq, Default, Clone)]
pub enum Rule {
    #[default]
    Any,
    All,
    Ord(Option<String>),
    Some(String),
}

impl Rule {
    pub fn parse(expr: &str) -> Result<Rule> {
        if expr.is_empty() {
            return Err(ActError::Runtime("for.by is empty".to_string()));
        }
        #[allow(unused_assignments)]
        let mut ret = Rule::Any;
        if expr == "any" {
            ret = Rule::Any;
        } else if expr.starts_with("some") {
            let re = Regex::new(r"some\((.*)\)$").unwrap();
            let caps = re.captures(&expr);

            if let Some(caps) = caps {
                let value = caps.get(1).map_or("", |m| m.as_str());
                ret = Rule::Some(value.to_string());
            } else {
                return Err(ActError::Runtime("cannot find some key for.by".to_string()));
            }
        } else if expr == "all" {
            ret = Rule::All;
        } else if expr.starts_with("ord") {
            ret = Rule::Ord(None);

            let re = Regex::new(r"ord\((.*)\)$").unwrap();
            let caps = re.captures(&expr);

            if let Some(caps) = caps {
                let value = caps.get(1).map_or("", |m| m.as_str());
                ret = Rule::Ord(Some(value.to_string()));
            }
        } else {
            return Err(ActError::Runtime(
                "for.by is error, it should be one of 'any', 'ord', 'ord(key)', 'all' and 'some(key)'".to_string(),
            ));
        }

        Ok(ret)
    }

    pub fn check(&self, ctx: &Context, f: &ActFor) -> Result<bool> {
        // check the env.cands, it is the precondition before checking the rule
        // if there is no env.cands, it means the group is in init stage and not generate the env.cands.
        let cands = ctx.var(consts::FOR_ACT_KEY_CANDS);
        if cands.is_none() {
            return Ok(false);
        }

        let children = ctx.task.children();
        let acts = children
            .iter()
            .filter(|iter| iter.node.id() == f.each_key())
            .collect::<Vec<_>>();
        match self {
            Rule::Any => {
                let ret = acts.iter().any(|t| t.state().is_success());
                Ok(ret)
            }
            Rule::All => {
                let ret = acts.iter().all(|act| act.state().is_success());
                Ok(ret)
            }
            Rule::Ord(_) => {
                let cands = cands
                    .unwrap()
                    .as_array()
                    .ok_or(ActError::Runtime(format!(
                        "env.{} is not an array in '{}'",
                        consts::FOR_ACT_KEY_CANDS,
                        ctx.task.task_id()
                    )))?
                    .iter()
                    .map(|iter| iter.as_str().unwrap().to_string())
                    .collect::<Vec<_>>();
                Ok(cands.len() == acts.len() && acts.iter().all(|act| act.state().is_success()))
            }
            Rule::Some(rule) => {
                // use the rule name as the key to confirm the result
                let is_pass = ctx
                    .task
                    .room
                    .get(rule)
                    .unwrap_or(false.into())
                    .as_bool()
                    .unwrap_or(false)
                    || acts.iter().all(|act| act.state() == TaskState::Success);

                if !is_pass {
                    // generate new some rule act
                    let acts = acts
                        .iter()
                        .filter(|iter| iter.node.id() == f.each_key())
                        .map(|iter| json!({ "id": iter.id, "uid": iter.room.get("uid"), "state": iter.state().to_string() }))
                        .collect::<Vec<_>>();
                    let mut inputs = Vars::new();
                    inputs.insert(consts::FOR_ACT_KEY_ACTS.to_string(), json!(acts));

                    let mut outputs = Vars::new();
                    outputs.insert(rule.to_string(), json!(null));

                    ctx.sched_act(rule, &f.tag(ctx), &inputs, &outputs);
                }

                Ok(is_pass)
            }
        }
    }

    pub fn next(&self, ctx: &Context, f: &ActFor) -> Result<bool> {
        if let Some(v) = ctx.var(consts::FOR_ACT_KEY_CANDS) {
            let cands = v
                .as_array()
                .ok_or(ActError::Runtime(format!(
                    "env.{} is not an array in '{}'",
                    consts::FOR_ACT_KEY_CANDS,
                    ctx.task.task_id()
                )))?
                .iter()
                .map(|iter| iter.as_str().unwrap().to_string())
                .collect::<Vec<_>>();

            if cands.len() == 0 {
                return Err(ActError::Runtime(format!(
                    "the env.cands is empty, please check the group's run script"
                )));
            }

            let no_completed_count = ctx
                .task
                .children()
                .iter()
                .filter(|iter| !iter.state().is_completed())
                .count();
            if no_completed_count > 0 {
                // directly return if there is none completed acts
                return Ok(true);
            }

            match self {
                Rule::All => {
                    for uid in cands.iter() {
                        let mut inputs = Vars::new();
                        inputs.insert(consts::FOR_ACT_KEY_UID.to_string(), json!(uid));
                        ctx.sched_act(&f.each_key(), &f.tag(ctx), &inputs, &Vars::new());
                    }
                }
                Rule::Any => {
                    let mut inputs = Vars::new();
                    inputs.insert(consts::FOR_ACT_KEY_CANDS.to_string(), json!(cands));
                    ctx.sched_act(&f.each_key(), &f.tag(ctx), &inputs, &Vars::new());
                }
                Rule::Some(_rule) => {
                    for uid in cands.iter() {
                        let mut inputs = Vars::new();
                        inputs.insert(consts::FOR_ACT_KEY_UID.to_string(), json!(uid));
                        ctx.sched_act(&f.each_key(), &f.tag(ctx), &inputs, &Vars::new());
                    }
                }
                Rule::Ord(rule) => {
                    if let Some(key) = rule {
                        let value = ctx.task.room.get(&key);
                        if value.is_none() {
                            let mut inputs = Vars::new();
                            inputs.insert(consts::FOR_ACT_KEY_CANDS.to_string(), json!(cands));

                            let mut outputs = Vars::new();
                            outputs.insert(consts::FOR_ACT_KEY_CANDS.to_string(), json!(null));
                            outputs.insert(key.to_string(), json!(null));

                            ctx.sched_act(&key, &f.tag(ctx), &inputs, &outputs);
                            return Ok(true);
                        }
                    }

                    let ord = ctx
                        .task
                        .room
                        .get(consts::FOR_ACT_KEY_ORD_INDEX)
                        .unwrap_or(0.into())
                        .as_i64()
                        .unwrap() as usize;

                    if let Some(uid) = cands.iter().skip(ord).next() {
                        let mut inputs = Vars::new();
                        inputs.insert(consts::FOR_ACT_KEY_UID.to_string(), json!(uid));
                        inputs.insert(consts::FOR_ACT_KEY_ORD_INDEX.to_string(), ord.into());
                        ctx.sched_act(&f.each_key(), &f.tag(ctx), &inputs, &Vars::new());

                        // order_index += 1
                        ctx.task
                            .room
                            .set(consts::FOR_ACT_KEY_ORD_INDEX, (ord + 1).into());
                    }
                }
            }
        } else {
            // if there is no 'cans' variable in env, it means the step did not finish the init
            if let Some(ref inits) = ctx.var(consts::FOR_ACT_KEY_USERS) {
                let values = utils::value_to_hash_map(inits)?;
                let users = f.calc(ctx, &values)?;
                ctx.set_var(
                    consts::FOR_ACT_KEY_CANDS,
                    users.into_iter().collect::<Vec<_>>().into(),
                );
                return self.next(ctx, f);
            }
        }

        Ok(true)
    }
}
