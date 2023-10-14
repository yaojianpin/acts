use std::collections::{BTreeSet, HashMap};

use crate::{ActError, Result};
use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::debug;

/// Org adapter trait
///
/// # Example
/// ```no_run
/// use acts::{OrgAdapter};
/// struct TestAdapter;
/// impl OrgAdapter for TestAdapter {
///     fn dept(&self, _value: &str) -> Vec<String> {
///         vec!["u1".to_string(), "u2".to_string()]
///     }
///     fn unit(&self, _value: &str) -> Vec<String> {
///         vec![
///             "u1".to_string(),
///             "u2".to_string(),
///             "u3".to_string(),
///             "u4".to_string(),
///         ]
///     }
///     fn relate(&self, _value: &str) -> Vec<String> {
///         vec!["p1".to_string()]
///     }
/// }
/// ```
pub trait OrgAdapter: Send + Sync {
    fn dept(&self, value: &str) -> Vec<String>;
    fn unit(&self, value: &str) -> Vec<String>;

    /// Get the users according to the relation
    ///
    /// { relateion } a relation string, like `user(123).d.d.owner`,  
    ///     d.d is current id's deparetment's parent deparentment, `owner` means the position
    ///     finally, it will return a users list
    fn relate(&self, value: &str) -> Vec<String>;
}

/// Role adapter trait
///
/// # Example
/// ```no_run
/// use acts::RoleAdapter;
/// struct TestAdapter;
/// impl RoleAdapter for TestAdapter {
///     fn role(&self, _name: &str) -> Vec<String> {
///         vec!["a1".to_string()]
///     }
/// }
/// ```
pub trait RoleAdapter: Send + Sync {
    fn role(&self, name: &str) -> Vec<String>;
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Candidate {
    Empty,
    User(String),
    Role(String),
    Unit(String),
    Dept(String),
    Relation(String),
    Group {
        op: Operation,
        items: Vec<Candidate>,
    },
    Set(Vec<Candidate>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum Operation {
    Union,
    Intersect,
    Difference,
}

impl Candidate {
    pub fn r#type(&self) -> String {
        match self {
            Candidate::Empty => "empty".to_string(),
            Candidate::User { .. } => "user".to_string(),
            Candidate::Role { .. } => "role".to_string(),
            Candidate::Unit { .. } => "unit".to_string(),
            Candidate::Dept { .. } => "dept".to_string(),
            Candidate::Relation { .. } => "rel".to_string(),
            Candidate::Group { .. } => "group".to_string(),
            Candidate::Set(..) => "set".to_string(),
        }
    }

    pub fn id(&self) -> Result<String> {
        match self {
            Candidate::User(v) => Ok(format!("user({v})")),
            Candidate::Role(v) => Ok(format!("role({v})")),
            Candidate::Unit(v) => Ok(format!("unit({v})")),
            Candidate::Dept(v) => Ok(format!("dept({v})")),
            Candidate::Relation(v) => Ok(format!("rel({v})")),
            _ => Err(ActError::Convert(format!(
                "only basic candidate can get id"
            ))),
        }
    }

    pub fn value(&self) -> Result<String> {
        match self {
            Candidate::User(v) => Ok(v.to_string()),
            Candidate::Role(v) => Ok(v.to_string()),
            Candidate::Unit(v) => Ok(v.to_string()),
            Candidate::Dept(v) => Ok(v.to_string()),
            Candidate::Relation(v) => Ok(v.to_string()),
            _ => Err(ActError::Convert(format!(
                "only basic candidate can get value"
            ))),
        }
    }

    pub fn parse(expr: &str) -> Result<Self> {
        debug!("candidate::parse {}", expr);
        if expr.is_empty() {
            Ok(Candidate::Empty)
        } else if expr.starts_with('{') && expr.ends_with('}') {
            let value: Value =
                serde_json::de::from_str(expr).map_err(|err| ActError::Convert(err.to_string()))?;
            Ok(value.into())
        } else if expr.starts_with('[') && expr.ends_with(']') {
            let value: Value =
                serde_json::de::from_str(expr).map_err(|err| ActError::Convert(err.to_string()))?;
            Ok(value.into())
        } else {
            Ok(Candidate::User(expr.to_string()))
        }
    }

    pub fn calculable(&self, cands: &mut Vec<Candidate>) -> bool {
        match self {
            Candidate::User { .. } => true,
            Candidate::Set(items) => {
                let mut result = true;
                for item in items {
                    result &= item.calculable(cands);
                }

                result
            }
            Candidate::Group { op: _, items } => {
                let mut result = true;
                for item in items {
                    result &= item.calculable(cands);
                }

                result
            }
            Candidate::Empty => false,
            cand => {
                cands.push(cand.clone());

                false
            }
        }
    }

    pub fn users(&self, values: &HashMap<String, Vec<String>>) -> Result<BTreeSet<String>> {
        let mut ret = BTreeSet::new();

        match self {
            Candidate::User(name) => {
                ret.insert(name.to_string());
            }
            Candidate::Set(items) => {
                let mut result = BTreeSet::new();
                for item in items {
                    let users = item.users(values)?;
                    result = result.union(&users).cloned().collect();
                }
                ret.extend(result);
            }
            Candidate::Group { op, items } => match op {
                Operation::Union => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.users(values)?;
                        result = result.union(&users).cloned().collect();
                    }
                    ret.extend(result);
                }
                Operation::Intersect => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.users(values)?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.intersection(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
                Operation::Difference => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.users(values)?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.difference(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
            },

            v => {
                let id = v.id()?;
                if let Some(users) = values.get(&id) {
                    for v in users.iter() {
                        ret.insert(v.to_string());
                    }
                } else {
                    return Err(ActError::Runtime(format!(
                        "cannot calculate users by {}",
                        id
                    )));
                }
            }
        }

        Ok(ret)
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Operation::Union => "union",
            Operation::Intersect => "intersect",
            Operation::Difference => "difference",
        };

        f.write_str(value)
    }
}

impl From<&str> for Operation {
    fn from(value: &str) -> Self {
        match value {
            "intersect" => Operation::Intersect,
            "difference" => Operation::Difference,
            "union" | _ => Operation::Union,
        }
    }
}

impl Into<Dynamic> for Candidate {
    fn into(self) -> Dynamic {
        let value: Value = self.into();
        let json = value.to_string();
        Dynamic::from(json)
    }
}

impl From<&Value> for Candidate {
    fn from(value: &Value) -> Self {
        value.clone().into()
    }
}

impl From<Value> for Candidate {
    fn from(value: Value) -> Self {
        match &value {
            // set single string to Candidate::User
            Value::String(v) => Candidate::User(v.to_string()),
            Value::Array(arr) => {
                let mut set = Vec::new();
                for item in arr {
                    set.push(item.into());
                }
                Candidate::Set(set)
            }
            Value::Object(obj) => {
                let Some(t) = obj.get("type") else {
                    return Candidate::Empty;
                };
                if !t.is_string() {
                    return Candidate::Empty;
                }

                match t.as_str().unwrap() {
                    "user" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::User(v.to_string())
                    }
                    "role" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::Role(v.to_string())
                    }
                    "unit" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::Unit(v.to_string())
                    }
                    "dept" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::Dept(v.to_string())
                    }
                    "rel" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::Relation(v.to_string())
                    }
                    "group" => {
                        let op: Operation = obj.get("op").unwrap().as_str().unwrap().into();
                        let items = obj.get("items").unwrap().as_array().unwrap();
                        let items: Vec<Candidate> =
                            items.iter().map(|item| item.clone().into()).collect();
                        Candidate::Group { op, items }
                    }
                    "set" => {
                        let items = obj.get("items").unwrap().as_array().unwrap();
                        let items: Vec<Candidate> =
                            items.iter().map(|item| item.clone().into()).collect();
                        Candidate::Set(items)
                    }
                    _ => Candidate::Empty,
                }
            }
            _ => Candidate::Empty,
        }
    }
}

impl Into<Value> for Candidate {
    fn into(self) -> Value {
        (&self).into()
    }
}

impl Into<Value> for &Candidate {
    fn into(self) -> Value {
        match self {
            Candidate::Group { op, items } => {
                let mut cands: Vec<Value> = Vec::new();
                for v in items {
                    cands.push(v.into());
                }
                json!({ "type": "group", "op": op.to_string(), "items": cands })
            }
            Candidate::Set(set) => {
                let mut cands: Vec<Value> = Vec::new();
                for v in set {
                    cands.push(v.into());
                }
                json!({ "type": "set", "items": cands })
            }
            Candidate::User(value) => {
                json!({ "type": "user", "value": value })
            }
            Candidate::Unit(value) => {
                json!({ "type": "unit",  "value": value })
            }
            Candidate::Dept(value) => {
                json!({ "type": "dept",  "value": value })
            }
            Candidate::Role(value) => {
                json!({ "type": "role", "value": value })
            }
            Candidate::Relation(value) => {
                json!({ "type": "rel", "value": value })
            }
            Candidate::Empty => Value::Null,
        }
    }
}
