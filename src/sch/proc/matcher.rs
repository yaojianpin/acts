use crate::{sch::Task, ActError, ActResult, Context, Step, TaskState};
use regex::Regex;

#[derive(Default, Clone)]
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
        let mut ret = Matcher::Empty;
        if expr.starts_with("one") {
            ret = Matcher::One;
        } else if expr.starts_with("any") {
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
        } else if expr.starts_with("all") {
            ret = Matcher::All;
        } else if expr.starts_with("ord") {
            ret = Matcher::Ord(None);

            let re = Regex::new(r"ord\((.*)\)$").unwrap();
            let caps = re.captures(&expr);

            if let Some(caps) = caps {
                let value = caps.get(1).map_or("", |m| m.as_str());
                ret = Matcher::Ord(Some(value.to_string()));
            }
        }

        ret
    }

    pub fn is_pass(&self, step: &Step, ctx: &Context) -> ActResult<bool> {
        match self {
            Matcher::Empty | Matcher::Error => {
                Err(ActError::SubjectError("matcher is empty".to_string()))
            }
            Matcher::One => {
                if let Some(act) = step.acts().get(0) {
                    return Ok(act.state() == TaskState::Success);
                }

                Ok(false)
            }
            Matcher::Any => {
                let mut ret = false;
                step.acts().iter().for_each(|act| {
                    let state = act.state();
                    if state == TaskState::Success {
                        ret = true;
                    } else {
                        act.set_state(&TaskState::Skip);
                    }
                });

                Ok(ret)
            }
            Matcher::All => {
                let ret = step
                    .acts()
                    .iter()
                    .all(|act| act.state() == TaskState::Success);

                Ok(ret)
            }
            Matcher::Ord(_) => {
                let ord = step.ord() + 1;
                let len = step.candidates().len();
                let ret = ord == len;
                if !ret {
                    if let Some(act) = step.candidates().get(ord) {
                        act.set_state(&TaskState::WaitingEvent);
                        step.set_ord(ord);
                        step.push_act(&act);

                        let task = Task::Act(act.id(), act.clone());
                        ctx.proc.tree.push_act(&task, &act.step_task_id);
                        ctx.send_message(&act.owner, &task);
                    }
                }

                Ok(ret)
            }
            Matcher::Some(rule) => match ctx.proc.scher.some(&rule, step, ctx) {
                Ok(ret) => {
                    if ret {
                        step.acts().iter().for_each(|act| {
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
