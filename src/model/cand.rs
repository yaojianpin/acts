use crate::{ActError, Result};
use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
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
    Value(String),
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
    Except,
}

impl Candidate {
    pub fn is_empty(&self) -> bool {
        match self {
            Candidate::Empty => true,
            _ => false,
        }
    }
    pub fn r#type(&self) -> String {
        match self {
            Candidate::Empty => "empty".to_string(),
            Candidate::Value { .. } => "value".to_string(),
            Candidate::Group { .. } => "group".to_string(),
            Candidate::Set(..) => "set".to_string(),
        }
    }

    pub fn id(&self) -> Result<String> {
        match self {
            Candidate::Value(v) => Ok(format!("value({v})")),
            _ => Err(ActError::Convert(format!(
                "only basic candidate can get id"
            ))),
        }
    }

    pub fn value(&self) -> Result<String> {
        match self {
            Candidate::Value(v) => Ok(v.to_string()),
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
            Ok(Candidate::Value(expr.to_string()))
        }
    }

    pub fn values(&self) -> Result<BTreeSet<String>> {
        let mut ret = BTreeSet::new();

        match self {
            Candidate::Empty => {}
            Candidate::Value(value) => {
                ret.insert(value.to_string());
            }
            Candidate::Set(items) => {
                let mut result = BTreeSet::new();
                for item in items {
                    let users = item.values()?;
                    result = result.union(&users).cloned().collect();
                }
                ret.extend(result);
            }
            Candidate::Group { op, items } => match op {
                Operation::Union => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.values()?;
                        result = result.union(&users).cloned().collect();
                    }
                    ret.extend(result);
                }
                Operation::Intersect => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.values()?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.intersection(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
                Operation::Except => {
                    let mut result = BTreeSet::new();
                    for item in items {
                        let users = item.values()?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.difference(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
            },
            // v => {
            //     let id = v.id()?;
            //     if let Some(values) = values.get(&id) {
            //         for v in values.iter() {
            //             ret.insert(v.to_string());
            //         }
            //     } else {
            //         return Err(ActError::Runtime(format!(
            //             "cannot calculate value by {}",
            //             id
            //         )));
            //     }
            // }
        }

        Ok(ret)
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Operation::Union => "union",
            Operation::Intersect => "intersect",
            Operation::Except => "except",
        };

        f.write_str(value)
    }
}

impl From<&str> for Operation {
    fn from(value: &str) -> Self {
        match value {
            "intersect" => Operation::Intersect,
            "except" => Operation::Except,
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
            Value::String(v) => Candidate::Value(v.to_string()),
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
                    "value" => {
                        let v = obj.get("value").unwrap().as_str().unwrap();
                        Candidate::Value(v.to_string())
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
            Candidate::Value(value) => {
                json!({ "type": "value", "value": value })
            }
            Candidate::Empty => Value::Null,
        }
    }
}
