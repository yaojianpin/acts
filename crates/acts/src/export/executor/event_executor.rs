use crate::{
    ActError, EventInfo, Principal, Result, TriggerKind, Vars, Workflow, data,
    query::Query,
    scheduler::Runtime,
    store::PageData,
    utils::{self, consts},
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tracing::instrument;

/// `evt:ls` — list deployed triggers.
pub(crate) const LS: &str = "evt:ls";
/// `evt:get` — read one deployed trigger.
pub(crate) const GET: &str = "evt:get";
/// `evt:start` — fire a trigger, starting its model.
pub(crate) const START: &str = "evt:start";

#[derive(Clone)]
pub struct EventExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl EventExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    #[instrument(skip(self))]
    pub async fn list(&self, q: &Query) -> Result<PageData<EventInfo>> {
        self.principal.check(LS)?;
        super::list_infos(self.runtime.store().events(), q).await
    }

    #[instrument(skip(self))]
    pub async fn get(&self, id: &str) -> Result<EventInfo> {
        self.principal.check(GET)?;
        super::get_info(self.runtime.store().events(), id).await
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
    ///
    /// A trigger reaches a model's own start inputs, so the run it starts
    /// carries the *firing caller's* authority: a snapshot-backed model is
    /// read under the scopes of whoever fired the trigger. A `schedule`
    /// trigger has no caller at all and is fired by the runtime itself, which
    /// seals nothing — see [`crate::ScopePolicy::default`].
    #[instrument(skip(self, params), fields(event_id = %event_id))]
    pub async fn start(&self, event_id: &str, params: &JsonValue) -> Result<Option<Vars>> {
        self.principal.check(START)?;
        let store = self.runtime.store();
        let event = store.events().find(event_id).await?;

        let kind = TriggerKind::parse(&event.kind);
        match kind {
            Some(TriggerKind::Manual) => {
                let workflow = self.workflow(&event).await?;
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                let pid = self
                    .runtime
                    .start(&workflow, self.start_options(payload_to_vars(payload)?))
                    .await?;
                Ok(Some(Vars::new().with(consts::PROCESS_ID, pid)))
            }
            Some(TriggerKind::Chat) => {
                let workflow = self.workflow(&event).await?;
                let mut start_params = Vars::new();
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                if let Some(v) = payload.as_str() {
                    start_params.set(consts::ACT_PARAMS_KEY, v);
                }
                let pid = self
                    .runtime
                    .start(&workflow, self.start_options(start_params))
                    .await?;
                Ok(Some(Vars::new().with(consts::PROCESS_ID, pid)))
            }
            Some(TriggerKind::Hook) => {
                let workflow = self.workflow(&event).await?;
                let payload = if params.is_null() {
                    event.default_params()
                } else {
                    params.clone()
                };
                let inputs = self.start_options(payload_to_vars(payload)?);
                self.start_hook(&workflow, inputs).await
            }
            Some(TriggerKind::Schedule) => Err(ActError::Runtime(format!(
                "cannot start the schedule trigger '{event_id}' manually"
            ))),
            None => {
                // custom kind: a registered event package id
                self.start_package(&event, params).await
            }
        }
    }

    /// The firing caller's authority, sealed into the run the trigger starts.
    /// A start option would be readable (and forgeable) from the workflow; the
    /// private env key is not — see [`crate::utils::consts::PROC_OWNER`].
    fn start_options(&self, mut options: Vars) -> Vars {
        options.set(consts::PROC_OWNER, self.principal.scope_policy());
        options
    }

    async fn workflow(&self, event: &data::Event) -> Result<Workflow> {
        let model = self.runtime.store().models().find(&event.mid).await?;
        let model: crate::ModelInfo = model.into();
        let workflow = model.workflow()?;
        // Firing a trigger starts the model's run, so the model's resource
        // must be within the firing caller's grants — the same check a
        // deploy and a proc:start run.
        self.principal.check_rn(&workflow.rn)?;
        Ok(workflow)
    }

    /// custom kinds — a registered package that exposes the non-context
    /// `start` entry (as `ActPackageCatalog::Event` used to)
    async fn start_package(&self, event: &data::Event, params: &JsonValue) -> Result<Option<Vars>> {
        let options = Vars::new().with(consts::MODEL_ID, &event.mid);
        let mut params = params.clone();
        if params.is_null() {
            params = serde_json::from_str(&event.params)
                .map_err(|err| ActError::Convert(format!("failed to deserialize params: {err}")))?;
        }
        let package = self
            .runtime
            .package()
            .create(&event.kind, self.runtime.config())?;
        let ret = package.start(&self.runtime, &params, &options).await?;
        Ok(ret)
    }

    async fn start_hook(&self, workflow: &Workflow, inputs: Vars) -> Result<Option<Vars>> {
        let rt = self.runtime.clone();

        let pid = utils::longid();
        let mut options = inputs;
        options.set(consts::PROCESS_ID, pid.as_str());

        let (sig, sig_c, sig_e) = crate::Signal::new(Vars::new()).triple();
        let key_c = format!("evt-hook-complete-{pid}");
        let key_e = format!("evt-hook-error-{pid}");

        let pid_c = pid.clone();
        rt.emitter().on_complete(&key_c, move |e| {
            let pid_c = pid_c.clone();
            let sig_c = sig_c.clone();
            async move {
                if e.pid == pid_c {
                    sig_c.send(e.outputs.clone());
                }
            }
        });
        let pid_e = pid.clone();
        rt.emitter().on_error(&key_e, move |e| {
            let pid_e = pid_e.clone();
            let sig_e = sig_e.clone();
            async move {
                if e.pid == pid_e {
                    sig_e.send(e.outputs.clone());
                }
            }
        });

        rt.start(workflow, options).await?;

        let ret = sig.recv().await;
        // neutralize the watchers; they are keyed per pid and no longer fire
        rt.emitter().on_complete(&key_c, |_| async {});
        rt.emitter().on_error(&key_e, |_| async {});
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Principal};

    /// A missing payload starts with no options, an object is the options,
    /// and a scalar is malformed input — not silently dropped.
    #[test]
    fn a_trigger_payload_becomes_the_start_options() {
        assert!(payload_to_vars(JsonValue::Null).unwrap().is_empty());
        let vars = payload_to_vars(serde_json::json!({"a": 1})).unwrap();
        assert_eq!(vars.get::<i32>("a"), Some(1));
        let err = payload_to_vars(serde_json::json!(5)).unwrap_err();
        assert!(
            err.to_string().contains("invalid trigger payload"),
            "got: {err}"
        );
    }

    #[derive(Clone)]
    struct ProbePackage;

    #[async_trait::async_trait]
    impl crate::ActPackage for ProbePackage {
        fn new(_: &crate::Config) -> crate::Result<Self> {
            Ok(Self)
        }

        fn definition() -> crate::ActPackageDefinition {
            crate::ActPackageDefinition {
                id: "export.test.probe",
                name: "probe",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: serde_json::json!({}),
                options: None,
                run_as: crate::ActRunAs::Func,
                resources: vec![],
                catalog: crate::ActPackageCatalog::Event,
            }
        }

        async fn start(
            &self,
            _rt: &dyn crate::ActPackageRuntime,
            _params: &JsonValue,
            _options: &Vars,
        ) -> crate::Result<Option<Vars>> {
            Ok(Some(Vars::new().with("started", true)))
        }
    }

    /// A hook trigger answers the run's own outputs. A failing run answers
    /// through the error watcher instead of hanging.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hook_trigger_answers_a_failed_run() {
        let engine = Engine::builder().start().await.unwrap();
        let executor = engine.executor(&Principal::unrestricted());
        let model = Workflow::new()
            .with_id("evt-hook-error")
            .with_trigger(|trigger| trigger.with_id("event1").with_kind("hook"))
            .with_step(|step| {
                step.with_id("s1").with_uses(
                    "acts.core.action",
                    Vars::new().with("action", "error").with(
                        "options",
                        Vars::new().with(crate::utils::consts::ACT_ERR_CODE, "E1"),
                    ),
                )
            });
        executor.model().deploy(&model, None).await.unwrap();

        let ret = executor
            .evt()
            .start("evt-hook-error:event1", &JsonValue::Null)
            .await
            .unwrap()
            .expect("the hook answers after the run settles");
        assert_eq!(
            ret.get::<String>(crate::utils::consts::ACT_ERR_CODE),
            Some("E1".to_string()),
            "the error watcher answers the failed run's outputs: {ret}"
        );
    }

    /// Firing a custom-kind trigger with no payload reads the trigger's own
    /// stored params; an unreadable stored payload is a conversion error.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_custom_trigger_reads_its_stored_params() {
        let engine = Engine::builder()
            .add_package::<ProbePackage>()
            .start()
            .await
            .unwrap();
        let executor = engine.executor(&Principal::unrestricted());
        let model = Workflow::new()
            .with_id("evt-probe")
            .with_trigger(|trigger| {
                trigger
                    .with_id("event1")
                    .with_kind("export.test.probe")
                    .with_params_data(serde_json::json!({"p": 1}))
            })
            .with_step(|step| step.with_id("s1"));
        executor.model().deploy(&model, None).await.unwrap();

        let ret = executor
            .evt()
            .start("evt-probe:event1", &JsonValue::Null)
            .await
            .unwrap()
            .expect("the package answers");
        assert_eq!(ret.get::<bool>("started"), Some(true));

        // Corrupt the stored params: the null payload then asks the executor
        // to deserialize them, and the failure is reported as input.
        let store = engine.runtime().store();
        let mut event = store.events().find("evt-probe:event1").await.unwrap();
        event.params = "not json".to_string();
        store.events().update(&event).await.unwrap();

        let err = executor
            .evt()
            .start("evt-probe:event1", &JsonValue::Null)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("failed to deserialize params"),
            "got: {err}"
        );
    }
}
