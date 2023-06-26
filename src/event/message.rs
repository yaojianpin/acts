use serde::{Deserialize, Serialize};

use crate::{event::EventAction, sch::ActKind, utils, ActValue, Vars};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum MessageKind {
    Task,
    Act(ActKind),
    Notice,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub kind: MessageKind,
    pub event: EventAction,
    pub mid: String,
    pub topic: String,
    pub nkind: String,
    pub nid: String,
    pub pid: String,
    pub tid: String,
    pub key: Option<String>,
    pub vars: Vars,
}

#[derive(Clone, Debug)]
pub struct TaskMessage<'a> {
    pub event: &'a EventAction,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub pid: &'a str,
    pub tid: &'a str,
    pub vars: &'a Vars,
}

#[derive(Clone, Debug)]
pub struct UserMessage<'a> {
    pub event: &'a EventAction,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub pid: &'a str,
    pub aid: &'a str,
    pub uid: &'a str,
    pub vars: &'a Vars,
}

#[derive(Clone, Debug)]
pub struct CandidateMessage<'a> {
    pub event: &'a EventAction,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub pid: &'a str,
    pub aid: &'a str,
    pub cands: ActValue,
    pub matcher: &'a str,
    pub vars: &'a Vars,
}

#[derive(Clone, Debug)]
pub struct ActionMessage<'a> {
    pub event: &'a EventAction,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub pid: &'a str,
    pub aid: &'a str,
    pub action: &'a str,
    pub vars: &'a Vars,
}

#[derive(Clone, Debug)]
pub struct NoticeMessage<'a> {
    pub pid: &'a str,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub key: &'a str,
    pub vars: &'a Vars,
}

#[derive(Clone, Debug)]
pub struct SomeMessage<'a> {
    pub event: &'a EventAction,
    pub mid: &'a str,
    pub mname: &'a str,
    pub nkind: &'a str,
    pub nid: &'a str,
    pub pid: &'a str,
    pub aid: &'a str,
    pub some: &'a str,
    pub vars: &'a Vars,
}

impl std::fmt::Display for MessageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            MessageKind::Task => "task".to_string(),
            MessageKind::Act(act) => format!("act:{act}"),
            MessageKind::Notice => format!("notice"),
        };

        f.write_str(&name)
    }
}

impl From<&str> for MessageKind {
    fn from(value: &str) -> Self {
        match value {
            "task" => MessageKind::Task,
            "notice" => MessageKind::Notice,
            act => MessageKind::Act(ActKind::from(act.replace("act:", "").as_ref())),
        }
    }
}

impl Message {
    pub fn as_task_message(&self) -> Option<TaskMessage> {
        match &self.kind {
            MessageKind::Task => Some(TaskMessage {
                mid: &self.mid,
                mname: &self.topic,
                nkind: &self.nkind,
                nid: &self.nid,
                pid: &self.pid,
                event: &self.event,
                tid: &self.tid,
                vars: &self.vars,
            }),
            _ => None,
        }
    }

    pub fn as_user_message(&self) -> Option<UserMessage> {
        match &self.kind {
            MessageKind::Act(kind) => match kind {
                ActKind::User => Some(UserMessage {
                    mid: &self.mid,
                    mname: &self.topic,
                    nkind: &self.nkind,
                    nid: &self.nid,
                    pid: &self.pid,
                    event: &self.event,
                    aid: self.key.as_ref().unwrap(),
                    vars: &self.vars,
                    uid: self.vars.get("owner").unwrap().as_str().unwrap(),
                }),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn as_action_message(&self) -> Option<ActionMessage> {
        match &self.kind {
            MessageKind::Act(kind) => match kind {
                ActKind::Action => Some(ActionMessage {
                    mid: &self.mid,
                    mname: &self.topic,
                    nkind: &self.nkind,
                    nid: &self.nid,
                    pid: &self.pid,
                    event: &self.event,
                    aid: self.key.as_ref().unwrap(),
                    vars: &self.vars,
                    action: self.vars.get("action").unwrap().as_str().unwrap(),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_notice_message(&self) -> Option<NoticeMessage> {
        match &self.kind {
            MessageKind::Notice => Some(NoticeMessage {
                mid: &self.mid,
                mname: &self.topic,
                nkind: &self.nkind,
                nid: &self.nid,
                pid: &self.pid,
                vars: &self.vars,
                key: self.key.as_ref().unwrap(),
            }),
            _ => None,
        }
    }

    pub fn as_canidate_message(&self) -> Option<CandidateMessage> {
        match &self.kind {
            MessageKind::Act(kind) => match kind {
                ActKind::Candidate => {
                    let cand_value = self.vars.get(utils::consts::SUBJECT_CANDS).unwrap();
                    let cands = cand_value.clone().into();

                    let matcher = self
                        .vars
                        .get(utils::consts::SUBJECT_MATCHER)
                        .unwrap()
                        .as_str()
                        .unwrap();

                    Some(CandidateMessage {
                        mid: &self.mid,
                        mname: &self.topic,
                        nkind: &self.nkind,
                        nid: &self.nid,
                        pid: &self.pid,
                        event: &self.event,
                        aid: self.key.as_ref().unwrap(),
                        vars: &self.vars,
                        cands,
                        matcher,
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_some_message(&self) -> Option<SomeMessage> {
        match &self.kind {
            MessageKind::Act(kind) => match kind {
                ActKind::Some => Some(SomeMessage {
                    mid: &self.mid,
                    mname: &self.topic,
                    nkind: &self.nkind,
                    nid: &self.nid,
                    pid: &self.pid,
                    event: &self.event,
                    aid: self.key.as_ref().unwrap(),
                    vars: &self.vars,
                    some: self.vars.get("some").unwrap().as_str().unwrap(),
                }),
                _ => None,
            },
            _ => None,
        }
    }
}
