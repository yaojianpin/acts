use crate::{
    ActError, Context, Executor, Result, Vars,
    package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs},
    utils::{self, consts},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct SubflowPackage;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SubflowPackageParams {
    pub to: String,

    #[serde(default)]
    pub options: Vars,
}

impl ActPackage for SubflowPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.subflow",
            name: "Subflow",
            desc: "call a subflow",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M608 64c53.02 0 96 42.98 96 96v144c0 53.02-42.98 96-96 96h-64v80h160c53.02 0 96 42.98 96 96v48h64c53.02 0 96 42.98 96 96v144c0 53.02-42.98 96-96 96H672c-53.02 0-96-42.98-96-96V720c0-53.02 42.98-96 96-96h64v-48c0-17.496-14.042-31.713-31.47-31.996L704 544H320c-17.496 0-31.713 14.042-32 31.47V624h64c53.02 0 96 42.98 96 96v144c0 53.02-42.98 96-96 96H160c-53.02 0-96-42.98-96-96V720c0-53.02 42.98-96 96-96h64v-48c0-53.02 42.98-96 96-96h160v-80h-64c-53.02 0-96-42.98-96-96V160c0-53.02 42.98-96 96-96h192z m256 624H672c-17.496 0-31.713 14.042-31.996 31.47L640 720v144c0 17.496 14.042 31.713 31.47 31.996l0.53 0.004h192c17.496 0 31.713-14.042 31.996-31.47L896 864V720c0-17.496-14.042-31.713-31.47-31.996L864 688z m-512 0H160c-17.496 0-31.713 14.042-31.996 31.47L128 720v144c0 17.496 14.042 31.713 31.47 31.996l0.53 0.004h192c17.496 0 31.713-14.042 31.996-31.47L384 864V720c0-17.496-14.042-31.713-31.47-31.996L352 688z m256-560H416c-17.496 0-31.713 14.042-31.996 31.47L384 160v144c0 17.496 14.042 31.713 31.47 31.996l0.53 0.004h192c17.496 0 31.713-14.042 31.996-31.47L640 304V160c0-17.496-14.042-31.713-31.47-31.996L608 128z" fill="currentColor"></path></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "title": "Subflow ID",
                        "description": "The subflow id to call"
                    },
                    "options": {
                        "type": "object",
                        "title": "Options",
                        "default": {},
                        "description": "Options to pass to the subflow",
                        "additionalProperties": {
                            "type": ["string", "number", "array", "boolean", "null"]
                        }
                    }
                },
                "required": ["to"]
            }),
            options: Some(json!({
                "ui:order": ["to", "options"]
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn new(_: &crate::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    fn execute(&self, ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        let params =
            serde_json::from_value::<SubflowPackageParams>(params.clone()).map_err(|e| {
                ActError::Package(format!(
                    "invalid ActPackage({}) params: {}",
                    Self::definition().id,
                    e
                ))
            })?;
        let task = ctx.task();
        task.set_auto_complete(false);
        let executor = Executor::new(&ctx.runtime);

        let mut inputs = utils::fill_inputs(&params.options, ctx);
        inputs.set(consts::ACT_USE_PARENT_PROC_ID, &ctx.proc.id());
        inputs.set(consts::ACT_USE_PARENT_TASK_ID, &task.id);
        executor.proc().start(&params.to, inputs)?;

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
        let meta = super::SubflowPackage::definition();
        serde_json::from_value::<super::SubflowPackageParams>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
