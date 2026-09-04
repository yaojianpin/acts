use crate::{
    ActError, EventInfo, Result, TriggerKind, Vars, Workflow, data,
    query::Query,
    scheduler::Runtime,
    store::PageData,
    utils::{self, consts},
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct EventExecutor {
    runtime: Arc<Runtime>,
}

impl EventExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self))]
    pub fn list(&self, q: &Query) -> Result<PageData<EventInfo>> {
        match self.runtime.cache().store().events().query(q) {
            Ok(events) => Ok(PageData {
                count: events.count,
                page_size: events.page_size,
                page_count: events.page_count,
                page_num: events.page_num,
                rows: events.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self))]
    pub fn get(&self, id: &str) -> Result<EventInfo> {
        let event = &self.runtime.cache().store().events().find(id)?;
        Ok(event.into())
    }

    /// Fire a deployed trigger by its event id (`mid:trigger-id`).
    ///
    /// - `manual`: start the workflow with the payload (or the trigger's
    ///   default `params` when none is given) and return the process id —
    ///   a webhook-style URL trigger is just `manual` fired over HTTP by a
    ///   transport (e.g. `acts-plugin-web`'s `/hooks/{event-id}` route)
    /// - `chat`: start the workflow with the string payload as `params`
    /// - `hook`: start the workflow and block until it completes, returning
    ///   its outputs
    /// - `schedule`: cannot be fired manually — the engine timer fires it
    /// - custom kinds: dispatched through the registered event package
    pub fn start(&self, event_id: &str, params: &JsonValue) -> Result<Option<Vars>> {
        let store = self.runtime.cache().store();
        let event = store.events().find(event_id)?;

        let kind = TriggerKind::parse(&event.kind);
        match kind {
            Some(TriggerKind::Manual) => {
                let workflow = self.workflow(&event)?;
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                let proc = self.runtime.start(&workflow, payload_to_vars(payload)?)?;
                Ok(Some(Vars::new().with(consts::PROCESS_ID, proc.id())))
            }
            Some(TriggerKind::Chat) => {
                let workflow = self.workflow(&event)?;
                let mut start_params = Vars::new();
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                if let Some(v) = payload.as_str() {
                    start_params.set(consts::ACT_PARAMS_KEY, v);
                }
                let proc = self.runtime.start(&workflow, start_params)?;
                Ok(Some(Vars::new().with(consts::PROCESS_ID, proc.id())))
            }
            Some(TriggerKind::Hook) => {
                let workflow = self.workflow(&event)?;
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                let inputs = payload_to_vars(payload)?;
                self.start_hook(&workflow, inputs)
            }
            Some(TriggerKind::Schedule) => Err(ActError::Runtime(format!(
                "cannot start the schedule trigger '{event_id}' manually"
            ))),
            None => {
                // custom kind: a registered event package id
                self.start_package(&event, params)
            }
        }
    }

    fn workflow(&self, event: &data::Event) -> Result<Workflow> {
        let model = self.runtime.cache().store().models().find(&event.mid)?;
        let model: crate::ModelInfo = model.into();
        model.workflow()
    }

    /// custom kinds — a registered package that exposes the non-context
    /// `start` entry (as `ActPackageCatalog::Event` used to)
    fn start_package(&self, event: &data::Event, params: &JsonValue) -> Result<Option<Vars>> {
        let register = self
            .runtime
            .package()
            .get(&event.kind)
            .ok_or(ActError::Runtime(format!(
                "cannot find the trigger kind '{}'",
                event.kind
            )))?;

        let options = Vars::new().with(consts::MODEL_ID, &event.mid);
        let mut params = params.clone();
        if params.is_null() {
            params = serde_json::from_str(&event.params)
                .map_err(|err| ActError::Convert(format!("failed to deserialize params: {err}")))?;
        }
        let package = (register.create)(self.runtime.config())?;
        let ret = package.start(&self.runtime, &params, &options)?;
        Ok(ret)
    }

    /// kind=`hook`: start and wait for the process to finish, return outputs.
    /// Registers completion watchers *before* the start so a fast process
    /// cannot complete before the handlers exist.
    fn start_hook(&self, workflow: &Workflow, inputs: Vars) -> Result<Option<Vars>> {
        let rt = self.runtime.clone();

        let pid = utils::longid();
        let mut options = inputs;
        options.set(consts::PROCESS_ID, pid.as_str());

        let (sig, sig_c, sig_e) = crate::Signal::new(Vars::new()).triple();
        let key_c = format!("evt-hook-complete-{pid}");
        let key_e = format!("evt-hook-error-{pid}");

        let pid_c = pid.clone();
        rt.emitter().on_complete(&key_c, move |e| {
            if e.pid == pid_c {
                sig_c.send(e.outputs.clone());
            }
        });
        let pid_e = pid.clone();
        rt.emitter().on_error(&key_e, move |e| {
            if e.pid == pid_e {
                sig_e.send(e.outputs.clone());
            }
        });

        rt.start(workflow, options)?;

        let ret = crate::utils::sync::block_on(async move { sig.recv().await });
        // neutralize the watchers; they are keyed per pid and no longer fire
        rt.emitter().on_complete(&key_c, |_| {});
        rt.emitter().on_error(&key_e, |_| {});
        Ok(Some(ret))
    }
}

/// coerce a trigger payload into the process start options
fn payload_to_vars(payload: JsonValue) -> Result<Vars> {
    match payload {
        JsonValue::Null => Ok(Vars::new()),
        value => serde_json::from_value::<Vars>(value)
            .map_err(|e| ActError::Convert(format!("invalid trigger payload: {e}"))),
    }
}
