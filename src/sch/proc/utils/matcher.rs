use crate::{
    sch::{proc::ActKind, Context},
    utils::consts,
    ActError, ActResult, TaskState,
};
use regex::Regex;
use serde_json::{json, Value};

#[derive(Debug, PartialEq, Default, Clone)]
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
    pub fn parse(expr: &str) -> ActResult<Matcher> {
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

        Ok(ret)
    }

    pub fn check(&self, ctx: &Context) -> ActResult<bool> {
        let acts = ctx.task.acts();
        let mut acts = acts.iter().filter(|act| act.kind == ActKind::User);
        match self {
            Matcher::Empty | Matcher::Error => {
                Err(ActError::Runtime("subject matcher is empty".to_string()))
            }
            Matcher::One => match acts.next() {
                Some(act) => Ok(act.state() == TaskState::Success),
                None => Ok(false),
            },
            Matcher::Any => {
                let mut ret = false;
                acts.for_each(|t| {
                    let state = t.state();
                    if state == TaskState::Success {
                        ret = true;
                    }
                });

                Ok(ret)
            }
            Matcher::All => {
                let ret = acts.all(|act| act.state() == TaskState::Success);
                Ok(ret)
            }
            Matcher::Ord(_) => {
                let ord = ctx
                    .task
                    .env
                    .get(consts::SUBJECT_ORD_INDEX)
                    .unwrap()
                    .as_i64()
                    .unwrap();
                let len = acts.clone().count() as i64;
                Ok(ord == len)
            }
            Matcher::Some(rule) => {
                // use the rule name as the key to confirm the result
                let is_pass = ctx
                    .task
                    .env
                    .get(rule)
                    .unwrap_or(false.into())
                    .as_bool()
                    .unwrap_or(false)
                    || acts.all(|act| act.state() == TaskState::Success);
                Ok(is_pass)
            }
        }
    }
}

impl Into<Value> for Matcher {
    fn into(self) -> Value {
        match self {
            Matcher::Empty => Value::Null,
            Matcher::One => json!("one"),
            Matcher::Any => json!("any"),
            Matcher::All => json!("all"),
            Matcher::Ord(ord) => match ord {
                Some(ord_id) => json!(format!("ord({ord_id})")),
                None => json!("ord"),
            },
            Matcher::Some(some_id) => json!(format!("some({some_id})")),
            Matcher::Error => Value::Null,
        }
    }
}

impl From<&Value> for Matcher {
    fn from(value: &Value) -> Self {
        value.clone().into()
    }
}

impl From<Value> for Matcher {
    fn from(value: Value) -> Self {
        match value {
            Value::String(expr) => match Matcher::parse(&expr) {
                Ok(m) => m,
                Err(_) => Matcher::Error,
            },
            _ => Matcher::Empty,
        }
    }
}
