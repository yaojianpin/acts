use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{ActError, Result, Vars};

/// Built-in trigger kinds of `Workflow.on`.
pub const TRIGGER_MANUAL: &str = "manual";
pub const TRIGGER_CHAT: &str = "chat";
pub const TRIGGER_HOOK: &str = "hook";
pub const TRIGGER_SCHEDULE: &str = "schedule";

/// The typed view of a trigger's [`Trigger::kind`]. Any kind string that is
/// not one of these is a custom kind — a registered event package id — and is
/// fired through the package registry (see `EventExecutor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// fire once from the client/UI, return the process id
    Manual,
    /// fire with a chat (string) message, return the process id
    Chat,
    /// fire and wait until the process completes, return its outputs
    Hook,
    /// fire periodically by a cron expression (engine timer)
    Schedule,
}
impl TriggerKind {
    /// parse a built-in kind name
    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            TRIGGER_MANUAL => Some(Self::Manual),
            TRIGGER_CHAT => Some(Self::Chat),
            TRIGGER_HOOK => Some(Self::Hook),
            TRIGGER_SCHEDULE => Some(Self::Schedule),
            _ => None,
        }
    }

    /// the kind name (reverse of [`Self::parse`])
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => TRIGGER_MANUAL,
            Self::Chat => TRIGGER_CHAT,
            Self::Hook => TRIGGER_HOOK,
            Self::Schedule => TRIGGER_SCHEDULE,
        }
    }
}

/// A trigger declaration: who may start the flow, when, and with what input.
///
/// Replaces the previous `Act`-based `on` entries. A trigger only declares
/// the start surface of the workflow — it is never executed inside a process
/// like a step `Act`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Trigger {
    #[serde(default)]
    pub id: String,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    /// trigger type: `manual` / `chat` / `hook` / `schedule`,
    /// or a registered event package id for custom kinds
    #[serde(default)]
    pub kind: String,

    /// default start inputs merged in when the trigger fires, used when the
    /// caller passes no payload (manual/hook) or as the payload of
    /// every scheduled run
    #[serde(default)]
    pub params: JsonValue,

    /// kind=`schedule`: cron expression (6 fields: sec min hour day month dow)
    #[serde(default)]
    pub schedule: Option<String>,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,
}

impl Trigger {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_string();
        self
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_desc(mut self, desc: &str) -> Self {
        self.desc = desc.to_string();
        self
    }

    pub fn with_params_data(mut self, v: JsonValue) -> Self {
        self.params = v;
        self
    }

    pub fn with_params_vars<F: Fn(Vars) -> Vars>(mut self, build: F) -> Self {
        let vars = build(Vars::default());
        self.params = vars.into();
        self
    }

    pub fn with_schedule(mut self, cron: &str) -> Self {
        self.schedule = Some(cron.to_string());
        self
    }

    pub fn with_metadata<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.metadata.set(name, value);
        self
    }

    pub fn builtin_kind(&self) -> Option<TriggerKind> {
        TriggerKind::parse(&self.kind)
    }

    /// whether the kind is one of the engine built-ins
    pub fn is_builtin(&self) -> bool {
        self.builtin_kind().is_some()
    }

    /// model-level validation: id, kind and kind-specific config
    pub fn valid(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(ActError::Model("workflow event id is empty".to_string()));
        }
        if self.kind.is_empty() {
            return Err(ActError::Model(format!(
                "workflow event({}) kind is empty",
                self.id
            )));
        }
        if self.builtin_kind() == Some(TriggerKind::Schedule) {
            match &self.schedule {
                Some(schedule) if !schedule.trim().is_empty() => {
                    if let Err(err) = crate::scheduler::cron::Cron::parse(schedule) {
                        return Err(ActError::Model(format!(
                            "workflow event({}) invalid schedule '{schedule}': {err}",
                            self.id
                        )));
                    }
                }
                _ => {
                    return Err(ActError::Model(format!(
                        "workflow event({}) missing schedule for kind '{}'",
                        self.id, self.kind
                    )));
                }
            }
        }
        Ok(())
    }
}
