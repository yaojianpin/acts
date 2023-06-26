use std::collections::HashSet;

use crate::{ActError, ActResult};
use rhai::Dynamic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Org adapter trait
///
/// # Example
/// ```no_run
/// use acts::{OrgAdapter};
/// struct TestAdapter;
/// impl OrgAdapter for TestAdapter {
///     fn dept(&self, _name: &str) -> Vec<String> {
///         vec!["u1".to_string(), "u2".to_string()]
///     }
///     fn unit(&self, _name: &str) -> Vec<String> {
///         vec![
///             "u1".to_string(),
///             "u2".to_string(),
///             "u3".to_string(),
///             "u4".to_string(),
///         ]
///     }
///     fn relate(&self, _id_type: &str, _id: &str, _relation: &str) -> Vec<String> {
///         vec!["p1".to_string()]
///     }
/// }
/// ```
pub trait OrgAdapter: Send + Sync {
    fn dept(&self, name: &str) -> Vec<String>;
    fn unit(&self, name: &str) -> Vec<String>;

    /// Get the users according to the relation
    ///
    /// { id_type } the id type, maybe user, depart, or unit, it can be judged by implementation
    /// { id } related id
    /// { relateion } a relation string, like `d.d.owner`,  
    ///     d.d is current id's deparetment's parent deparentment, `owner` means the position
    ///     finally, it will return a users list
    fn relate(&self, id_type: &str, id: &str, relation: &str) -> Vec<String>;
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
    Relation {
        id: String,
        rel: String,
    },
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
    pub fn parse(expr: &str) -> ActResult<Self> {
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

    pub fn calced(&self) -> bool {
        match self {
            Candidate::User(_) => true,
            Candidate::Set(items) => {
                let mut result = true;
                for item in items {
                    result &= item.calced();
                }

                result
            }
            Candidate::Group { op: _, items } => {
                let mut result = true;
                for item in items {
                    result &= item.calced();
                }

                result
            }
            _ => false,
        }
    }

    pub fn users(&self) -> ActResult<HashSet<&str>> {
        let mut ret = HashSet::new();

        match self {
            Candidate::User(id) => {
                ret.insert(id.as_str());
            }
            Candidate::Set(items) => {
                let mut result = HashSet::new();
                for item in items {
                    let users = item.users()?;
                    result = result.union(&users).cloned().collect();
                }
                ret.extend(result);
            }
            Candidate::Group { op, items } => match op {
                Operation::Union => {
                    let mut result = HashSet::new();
                    for item in items {
                        let users = item.users()?;
                        result = result.union(&users).cloned().collect();
                    }

                    ret.extend(result);
                }
                Operation::Intersect => {
                    let mut result = HashSet::new();
                    for item in items {
                        let users = item.users()?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.intersection(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
                Operation::Difference => {
                    let mut result = HashSet::new();
                    for item in items {
                        let users = item.users()?;
                        if result.len() == 0 {
                            result.extend(users);
                        } else {
                            result = result.difference(&users).cloned().collect();
                        }
                    }

                    ret.extend(result);
                }
            },
            _ => {
                return Err(ActError::Model(format!(
                    "the candidate does not support to extract users"
                )))
            }
        }

        Ok(ret)
    }

    pub fn calc<T: RoleAdapter + OrgAdapter>(&self, adapter: &T) -> ActResult<Self> {
        let ret = match self {
            Candidate::Role(id) => {
                let users = adapter
                    .role(id)
                    .iter()
                    .map(|uid| Candidate::User(uid.clone()))
                    .collect();

                Candidate::Set(users)
            }
            Candidate::Unit(id) => {
                let users = adapter
                    .unit(id)
                    .iter()
                    .map(|uid| Candidate::User(uid.clone()))
                    .collect();

                Candidate::Set(users)
            }
            Candidate::Dept(id) => {
                let users = adapter
                    .dept(id)
                    .iter()
                    .map(|uid| Candidate::User(uid.clone()))
                    .collect();

                Candidate::Set(users)
            }
            Candidate::Relation { id, rel } => {
                let users = adapter
                    .relate("user", id, rel)
                    .iter()
                    .map(|uid| Candidate::User(uid.clone()))
                    .collect();
                Candidate::Set(users)
            }
            Candidate::Group { op, items } => {
                let mut list = Vec::new();
                for item in items.iter() {
                    let cand = item.calc(adapter)?;
                    list.push(cand);
                }

                Candidate::Group {
                    op: op.clone(),
                    items: list,
                }
            }
            Candidate::Set(items) => {
                let mut list = Vec::new();
                for item in items.iter() {
                    let cand = item.calc(adapter)?;
                    list.push(cand);
                }
                Candidate::Set(list)
            }
            others => others.clone(),
        };

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

// impl std::fmt::Display for Candidate {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         let ret: Value = self.into();
//         f.write_str(&ret.to_string())
//     }
// }

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
            Value::String(id) => Candidate::User(id.to_string()),
            Value::Object(obj) => {
                let Some(t) = obj.get("type") else {
                    return Candidate::Empty;
                };
                if !t.is_string() {
                    return Candidate::Empty;
                }

                match t.as_str().unwrap() {
                    "user" => {
                        let id = obj.get("id").unwrap().as_str().unwrap();
                        Candidate::User(id.to_string())
                    }
                    "role" => {
                        let id = obj.get("id").unwrap().as_str().unwrap();
                        Candidate::Role(id.to_string())
                    }
                    "unit" => {
                        let id = obj.get("id").unwrap().as_str().unwrap();
                        Candidate::Unit(id.to_string())
                    }
                    "dept" => {
                        let id = obj.get("id").unwrap().as_str().unwrap();
                        Candidate::Dept(id.to_string())
                    }
                    "rel" => {
                        let rel = obj.get("rel").unwrap().as_str().unwrap();
                        let uid = obj.get("id").unwrap().as_str().unwrap();
                        Candidate::Relation {
                            id: uid.to_string(),
                            rel: rel.to_string(),
                        }
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
            Candidate::User(id) => {
                json!({ "type": "user", "id": id })
            }
            Candidate::Unit(id) => {
                json!({ "type": "unit", "id": id })
            }
            Candidate::Dept(id) => {
                json!({ "type": "dept", "id": id })
            }
            Candidate::Role(id) => {
                json!({ "type": "role", "id": id })
            }
            Candidate::Relation { id: uid, rel } => {
                json!({ "type": "rel", "id": uid, "rel": rel })
            }

            Candidate::Empty => Value::Null,
        }
    }
}
