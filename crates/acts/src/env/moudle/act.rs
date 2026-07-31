use super::super::ActModule;
use crate::{ActError, Context, Result, Vars, env::value::ActJsValue};
use rquickjs::{CatchResultExt, Module as JsModule};

pub struct ActJsModule;
impl ActJsModule {
    pub fn new() -> Self {
        Self
    }

    pub fn vars(&self) -> Option<Vars> {
        if let Ok(ctx) = Context::current() {
            return Some(ctx.task().vars());
        }
        None
    }
}

#[allow(clippy::module_inception)]
#[rquickjs::module(rename_vars = "camelCase")]
mod act {
    use crate::{Context, TimeoutLimit, Vars, env::value::ActJsValue, utils::consts};

    #[rquickjs::function]
    pub fn get_act_value(name: String) -> Option<ActJsValue> {
        Context::with(|ctx| {
            if let Some(v) = ctx.task().find(&name) {
                let v = ActJsValue::new(v);
                return Some(v);
            }
            None
        })
    }

    #[rquickjs::function]
    pub fn set_act_value(name: String, value: ActJsValue) {
        Context::with(|ctx| {
            let vars = Vars::new().with(&name, value.inner());
            ctx.task().update_data(&vars);
        })
    }

    #[rquickjs::function]
    pub fn set_process_var(name: String, value: ActJsValue) {
        Context::with(|ctx| {
            let vars = Vars::new().with(&name, value.inner());
            ctx.proc.set_data(&vars);
        })
    }

    #[rquickjs::function]
    pub fn get_act_inputs() -> ActJsValue {
        Context::with(|ctx| ctx.task().inputs().into())
    }

    #[rquickjs::function]
    pub fn get_act_data() -> ActJsValue {
        Context::with(|ctx| ctx.task().data().into())
    }

    #[rquickjs::function]
    pub fn err_code() -> Option<String> {
        Context::with(|ctx| ctx.task().data().get::<String>(consts::ACT_ERR_CODE))
    }

    #[rquickjs::function]
    pub fn cost() -> i64 {
        Context::with(|ctx| {
            let task = ctx.task();
            // 1. current task has a user-set TASK_COST
            if let Some(cost) = task.data().get::<i64>(consts::TASK_COST) {
                return cost;
            }
            // 2. check parent chain for TASK_COST in data
            let mut p = task.parent();
            while let Some(parent) = p {
                if let Some(cost) = parent.data().get::<i64>(consts::TASK_COST) {
                    return cost;
                }
                p = parent.parent();
            }
            // 3. fallback: own runtime cost
            ctx.task().cost()
        })
    }

    #[rquickjs::function]
    pub fn cost_in(min: String, max: Option<String>) -> bool {
        Context::with(|ctx| {
            let task = ctx.task();
            let mut cost = task.cost();
            if let Some(v) = task.data().get::<i64>(consts::TASK_COST) {
                cost = v;
            } else {
                // check parent chain for TASK_COST in data
                let mut p = task.parent();
                while let Some(parent) = p {
                    if let Some(v) = parent.data().get::<i64>(consts::TASK_COST) {
                        cost = v;
                        break;
                    }
                    p = parent.parent();
                }
            }

            let min_timeout = TimeoutLimit::parse(&min).unwrap_or_default();

            let mut ret = cost >= min_timeout.as_secs() * 1000;
            if ret && let Some(v) = &max {
                let max_timeout = TimeoutLimit::parse(v).unwrap_or_default();
                ret &= cost < max_timeout.as_secs() * 1000;
            }

            ret
        })
    }
}

impl ActModule for ActJsModule {
    fn init(&self, ctx: &rquickjs::Ctx<'_>) -> Result<()> {
        JsModule::declare_def::<js_act, _>(ctx.clone(), "@acts/act").unwrap();

        if let Some(vars) = self.vars() {
            for (key, value) in &vars {
                ctx.globals().set(&key, ActJsValue::new(value))?;
            }
        }

        let source = r#"
        import { get_act_value, set_act_value, set_process_var, get_act_inputs, get_act_data, err_code, cost, cost_in } from '@acts/act';

        globalThis.$get = get_act_value;
        globalThis.$set = set_act_value;
        globalThis.$inputs = get_act_inputs;
        globalThis.$data = get_act_data;
        globalThis.$set_process_var = set_process_var;
        globalThis.$ecode = err_code;
        globalThis.$cost = cost
        globalThis.$cost_in = (min, max) => {
            return cost_in(min, max);
        }
        "#;
        let _ = JsModule::evaluate(ctx.clone(), "@acts/act", source)
            .catch(ctx)
            .map_err(|err| ActError::Script(err.to_string()))?;

        Ok(())
    }
}
