use super::Job;
use crate::{sch::NodeTree, ActError, ModelBase, Result, Vars};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowActionOn {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub nkind: Option<String>,
    #[serde(default)]
    pub nid: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowAction {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub inputs: Vars,
    #[serde(default)]
    pub outputs: Vars,
    #[serde(default)]
    pub on: Vec<WorkflowActionOn>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub tag: String,

    #[serde(default)]
    pub jobs: Vec<Job>,

    #[serde(default)]
    pub env: Vars,

    #[serde(default)]
    pub outputs: Vars,

    #[serde(default)]
    pub actions: Vec<WorkflowAction>,

    #[serde(default)]
    ver: u32,
}

impl Workflow {
    pub fn from_yml(s: &str) -> Result<Self> {
        let workflow = serde_yaml::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{}", e))),
        }
    }

    pub fn from_json(s: &str) -> Result<Self> {
        let workflow = serde_json::from_str::<Workflow>(s);
        match workflow {
            Ok(v) => Ok(v),
            Err(e) => Err(ActError::Model(format!("{}", e))),
        }
    }

    pub fn set_env(&mut self, vars: &Vars) {
        for (name, value) in vars {
            self.env
                .entry(name.clone())
                .and_modify(|v| *v = value.clone())
                .or_insert(value.clone());
        }
    }

    pub fn print(&self) {
        let mut root = NodeTree::new();
        root.load(self).unwrap();
        root.print();
    }

    pub fn tree_output(&self) -> String {
        let mut root = NodeTree::new();
        root.load(self).unwrap();
        root.tree_output()
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        match self.jobs.iter().find(|job| job.id == id) {
            Some(job) => {
                // job.set_workflow(Box::new(self.clone()));
                Some(job)
            }
            None => None,
        }
    }

    pub fn set_id(&mut self, id: &str) {
        self.id = id.to_string();
    }

    pub fn set_ver(&mut self, ver: u32) {
        self.ver = ver;
    }

    pub fn to_yml<'a>(&self) -> Result<String> {
        match serde_yaml::to_string(self) {
            Ok(s) => Ok(s),
            Err(e) => Err(ActError::Model(e.to_string())),
        }
    }

    pub fn to_json<'a>(&self) -> Result<String> {
        match serde_json::to_string(self) {
            Ok(s) => Ok(s),
            Err(e) => Err(ActError::Model(e.to_string())),
        }
    }

    pub fn action(&self, id: &str) -> Option<&WorkflowAction> {
        self.actions.iter().find(|item| item.id == id)
    }

    pub fn valid(&self) -> Result<()> {
        let mut root = NodeTree::new();
        root.load(self)?;
        Ok(())
    }
}

impl ModelBase for Workflow {
    fn id(&self) -> &str {
        &self.id
    }
}
