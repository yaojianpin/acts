use crate::{sch::tree::NodeData, ActError, ActResult, Context, Step, TaskState};
use regex::Regex;

#[derive(Debug, Default, Clone)]
pub enum Matcher {
    #[default]
    Empty,
    One,
    Any,
    All,
    Ord(Option<String>),
    Some(String),
    Error,
}

impl Matcher {
    pub fn capture(expr: &str) -> Matcher {
        #[allow(unused_assignments)]
        let mut ret = Matcher::Empty;
        if expr == "one" {
            ret = Matcher::One;
        } else if expr == "any" {
            ret = Matcher::Any;
        } else if expr.starts_with("some") {
            let re = Regex::new(r"some\((.*)\)$").unwrap();
            let caps = re.captures(&expr);

            if let Some(caps) = caps {
                let value = caps.get(1).map_or("", |m| m.as_str());
                ret = Matcher::Some(value.to_string());
            } else {
                ret = Matcher::Error;
            }
        } else if expr == "all" {
            ret = Matcher::All;
        } else if expr.starts_with("ord") {
            ret = Matcher::Ord(None);

            let re = Regex::new(r"ord\((.*)\)$").unwrap();
            let caps = re.captures(&expr);

            if let Some(caps) = caps {
                let value = caps.get(1).map_or("", |m| m.as_str());
                ret = Matcher::Ord(Some(value.to_string()));
            }
        } else {
            ret = Matcher::Error;
        }

        ret
    }

    pub fn check(&self, step: &Step, ctx: &Context) -> ActResult<bool> {
        match self {
            Matcher::Empty | Matcher::Error => Err(ActError::RuntimeError(
                "subject matcher is empty".to_string(),
            )),
            Matcher::One => {
                let tasks = ctx.proc.children(&ctx.task);
                if let Some(act) = tasks.get(0) {
                    return Ok(act.state() == TaskState::Success);
                }

                Ok(false)
            }
            Matcher::Any => {
                let mut ret = false;
                let tasks = ctx.proc.children(&ctx.task);
                tasks.iter().for_each(|t| {
                    let state = t.state();
                    if state == TaskState::Success {
                        ret = true;
                    }
                });

                Ok(ret)
            }
            Matcher::All => {
                let tasks = ctx.proc.children(&ctx.task);
                let ret = tasks.iter().all(|act| act.state() == TaskState::Success);

                Ok(ret)
            }
            Matcher::Ord(_) => {
                let cands = &mut *step.cands();
                let ord = cands.ord + 1;
                let len = cands.acts.len();
                let ret = ord == len;
                if !ret {
                    if let Some(act) = cands.acts.get(ord) {
                        cands.ord = ord;
                        step.push_act(&act);

                        let data = NodeData::Act(act.clone());
                        let node = ctx.proc.tree.push_act(&data, &act.step_id);

                        ctx.sched_task(&node);
                    }
                }

                Ok(ret)
            }
            Matcher::Some(rule) => match ctx.proc.scher.some(&rule, step, ctx) {
                Ok(ret) => {
                    if ret {
                        let acts = ctx.proc.children(&ctx.task);
                        acts.iter().for_each(|act| {
                            let state = act.state();
                            if state != TaskState::Success {
                                act.set_state(&TaskState::Skip);
                            }
                        });
                    }

                    Ok(ret)
                }

                Err(err) => Err(err),
            },
        }
    }
}
