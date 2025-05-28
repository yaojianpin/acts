use super::super::ActModule;
use crate::{ActError, Result};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct StepModule;
impl StepModule {
    pub fn new() -> Self {
        Self
    }
}

#[allow(clippy::module_inception)]
#[rquickjs::module(rename_vars = "camelCase")]
mod step {
    use crate::{ActError, Context, Result, Vars, env::value::ActValue};

    #[rquickjs::function]
    pub fn get_step_value(nid: String, name: String) -> ActValue {
        Context::with(|ctx| {
            let tasks = ctx.task().proc().find_tasks(|task| task.node().id() == nid);
            if !tasks.is_empty() {
                let task = tasks.last().unwrap();

                return task
                    .with_data(|data| data.get(&name))
                    .map(ActValue::new)
                    .unwrap_or(ActValue::new(serde_json::Value::Null));
            }
            ActValue::new(serde_json::Value::Null)
        })
    }

    #[rquickjs::function]
    pub fn set_step_value(nid: String, name: String, value: ActValue) -> Result<()> {
        Context::with(|ctx| {
            let tasks = ctx.task().proc().find_tasks(|task| task.node().id() == nid);
            if !tasks.is_empty() {
                let task = tasks.last().unwrap();
                if task.state().is_completed() {
                    return Err(ActError::Script(format!(
                        "Task with nid '{}' is already completed, cannot set value",
                        nid
                    )));
                }
                task.update_data(&Vars::new().with(&name, value.inner()))
            }
            Ok(())
        })
    }

    #[rquickjs::function]
    pub fn get_steps() -> Vec<String> {
        if Context::current().is_err() {
            return vec![];
        }
        Context::with(|ctx| {
            ctx.task()
                .proc()
                .tasks()
                .iter()
                .filter(|task| task.is_kind(crate::NodeKind::Step))
                .map(|task| task.node().id().to_string())
                .collect()
        })
    }

    #[rquickjs::function]
    pub fn get_inputs(nid: String) -> ActValue {
        Context::with(|ctx| {
            let tasks = ctx.task().proc().find_tasks(|task| task.node().id() == nid);
            if !tasks.is_empty() {
                let task = tasks.last().unwrap();
                return task.inputs().into();
            }
            ActValue::new(serde_json::Value::Null)
        })
    }

    #[rquickjs::function]
    pub fn get_data(nid: String) -> ActValue {
        Context::with(|ctx| {
            let tasks = ctx.task().proc().find_tasks(|task| task.node().id() == nid);
            if !tasks.is_empty() {
                let task = tasks.last().unwrap();
                return task.data().into();
            }
            ActValue::new(serde_json::Value::Null)
        })
    }
}

impl ActModule for StepModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_step, _>(ctx.clone(), "@acts/step").unwrap();
        let source = r#"
        import { get_step_value, set_step_value, get_data, get_inputs, get_steps } from '@acts/step';

        const step = (id) => {
            if (typeof id !== 'string') {
                throw new Error('Step ID must be a string');
            }
            return {
                id, 
                data() { 
                    return get_data(this.id); 
                },
                inputs() { 
                    return get_inputs(this.id); 
                },
            };
        };
        const stepHandler = {
            get(target, prop, receiver) {
                if (typeof target[prop] === 'function' && target.hasOwnProperty(prop)) {
                    return target[prop].bind(target);
                }
                return get_step_value(target.id, prop);
            },
            set(target, prop, value, receiver) {
                set_step_value(target.id, prop, value);
                return true;
            },
        }

        for(let id of get_steps()) {
            globalThis[id] = new Proxy(step(id), stepHandler);
        }
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/step", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
