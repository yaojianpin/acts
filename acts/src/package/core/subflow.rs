use crate::{
    Context, Executor, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
    utils::{self, consts},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SubflowPackage {
    pub to: String,

    #[serde(default)]
    pub options: Vars,
}

impl ActPackage for SubflowPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.core.subflow",
            desc: "call a subflow",
            version: "0.1.0",
            icon: "icon-subflow",
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "options": { "type": "object"}
                }
            }),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl ActPackageFn for SubflowPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let task = ctx.task();
        task.set_auto_complete(false);

        let executor = Executor::new(&ctx.runtime);

        let mut inputs = utils::fill_inputs(&self.options, ctx);
        inputs.set(consts::ACT_USE_PARENT_PROC_ID, &ctx.proc.id());
        inputs.set(consts::ACT_USE_PARENT_TASK_ID, &task.id);
        executor.proc().start(&self.to, &inputs)?;

        Ok(None)
    }
}

inventory::submit!(ActPackageRegister::new::<SubflowPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_subflow_parse() {
        let params = r#"
        to: sub1
        options:
            a: abc
            b: 1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::SubflowPackage::meta();
        serde_json::from_value::<super::SubflowPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
