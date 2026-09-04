use crate::{
    ModelInfo, Result, Trigger, Workflow, data,
    query::{Expr, Filter, Query},
    scheduler::Runtime,
    store::PageData,
    utils::{self, consts},
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
pub struct ModelExecutor {
    runtime: Arc<Runtime>,
}

impl ModelExecutor {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self {
            runtime: rt.clone(),
        }
    }

    #[instrument(skip(self, model, view), fields(id = %model.id, name = %model.name))]
    pub fn deploy(&self, model: &Workflow, view: Option<&JsonValue>) -> Result<bool> {
        model.valid()?;

        let store = self.runtime.cache().store();
        let ret = store.deploy(model, view)?;
        self.deploy_triggers(&model.on, &model.id, &model.ver)?;

        Ok(ret)
    }

    #[instrument(skip(self, q))]
    pub fn list(&self, q: &Query) -> Result<PageData<ModelInfo>> {
        match self.runtime.cache().store().models().query(q) {
            Ok(models) => Ok(PageData {
                count: models.count,
                page_size: models.page_size,
                page_count: models.page_count,
                page_num: models.page_num,
                rows: models.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self), fields(id = %id))]
    pub fn get(&self, id: &str, fmt: &str) -> Result<ModelInfo> {
        match self.runtime.cache().store().models().find(id) {
            Ok(m) => {
                let mut model: ModelInfo = m.into();
                match fmt {
                    "tree" => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.tree_output();
                    }
                    "yaml" => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.to_yml()?;
                    }
                    _ => {
                        let workflow = Workflow::from_yml(&model.data)?;
                        model.data = workflow.to_json()?;
                    }
                }

                Ok(model)
            }
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self), fields(id = %id))]
    pub fn rm(&self, id: &str) -> Result<bool> {
        let store = self.runtime.cache().store();

        // find the model events and delete them
        let events = store
            .events()
            .query(&Query::new().filter(Filter::and().expr(Expr::eq(consts::MODEL_ID, id))))?;
        for evt in events.rows {
            store.events().delete(&evt.id)?;
        }

        // remove the model
        store.models().delete(id)
    }

    /// Reconcile the trigger rows of a model against the deployed model
    /// declaration:
    ///
    /// - create missing triggers,
    /// - update triggers whose declaration changed (name/kind/params/schedule),
    /// - delete rows that are no longer declared (stale entries from an older
    ///   version of the model).
    ///
    /// `schedule` triggers keep their `last_run`/`next_run` state across
    /// re-deploys unless the cron expression itself changed (then the next
    /// run is re-armed to fire on the next tick).
    fn deploy_triggers(&self, triggers: &[Trigger], mid: &str, ver: &str) -> Result<()> {
        let store = self.runtime.cache().store();
        let events = store.events();

        let existing = events
            .query(
                &Query::new()
                    .limit(1000)
                    .filter(Filter::and().expr(Expr::eq(consts::MODEL_ID, mid))),
            )?
            .rows;

        let mut declared: Vec<data::Event> = Vec::new();
        let mut keep: Vec<String> = Vec::new();
        for trigger in triggers {
            let event_id = format!("{}:{}", mid, trigger.id);
            keep.push(event_id.clone());
            declared.push(data::Event::from_trigger(trigger, mid, ver, &event_id)?);
        }

        for row in existing.iter() {
            if !keep.contains(&row.id) {
                // no longer declared — drop the stale trigger row
                events.delete(&row.id)?;
            }
        }

        for mut event in declared {
            match events.find(&event.id) {
                Ok(evt) => {
                    let changed = evt.name != event.name
                        || evt.kind != event.kind
                        || evt.params != event.params
                        || evt.schedule != event.schedule
                        || evt.ver != event.ver;
                    if !changed {
                        continue;
                    }
                    // keep the schedule run state unless the cron changed
                    event.last_run = evt.last_run;
                    event.next_run = if evt.schedule == event.schedule {
                        evt.next_run
                    } else if event.schedule.is_some() {
                        utils::time::time_millis()
                    } else {
                        0
                    };
                    events.update(&event)?;
                }
                Err(_) => {
                    // new trigger: arm `schedule` rows on the next tick
                    if event.schedule.is_some() {
                        event.next_run = utils::time::time_millis();
                    }
                    events.create(&event)?;
                }
            }
        }

        Ok(())
    }
}
