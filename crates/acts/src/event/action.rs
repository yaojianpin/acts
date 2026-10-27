use crate::Vars;
use crate::event::EventAction;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub pid: String,
    pub tid: String,
    pub event: EventAction,
    pub options: Vars,
    /// The authenticated caller the server attributes this decision to. It is
    /// filled by the executor from the [`Principal`](crate::Principal), never
    /// read from the client's options: an embedder calling the runtime
    /// directly leaves it `None`, which keeps the pre-assignment behavior.
    #[serde(default)]
    pub subject: Option<String>,
    /// The assignment revision the client decided against. It guards a
    /// decision racing a transfer; a mismatch is refused, and an absent value
    /// keeps the pre-assignment clients working.
    #[serde(default)]
    pub rev: Option<u64>,
}

impl Action {
    pub fn new(pid: &str, tid: &str, event: EventAction, options: Vars) -> Self {
        Self {
            id: String::new(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            event,
            options,
            subject: None,
            rev: None,
        }
    }

    /// Attribute the action to an authenticated subject (the executor's
    /// server-side fill, see [`Self::subject`]).
    pub fn with_subject(mut self, subject: Option<String>) -> Self {
        self.subject = subject;
        self
    }

    /// Carry the client's assignment revision for the kernel's compare (see
    /// [`Self::rev`]).
    pub fn with_rev(mut self, revision: Option<u64>) -> Self {
        self.rev = revision;
        self
    }
}
