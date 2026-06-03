use crate::{
    ActError, ModelInfo, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
    utils::consts,
};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct ManualEventPackage(Option<Vars>);

impl ActPackage for ManualEventPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.event.manual",
            name: "Manual",
            desc: "do an event by manual",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-hand-icon lucide-hand"><path d="M18 11V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2"/><path d="M14 10V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v2"/><path d="M10 10.5V6a2 2 0 0 0-2-2a2 2 0 0 0-2 2v8"/><path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"/></svg>"#,
            doc: "",
            in_schema: json!({
                "type": "object",
                "properties": {
                    "params": {
                        "type": "object",
                        "title": "Parameters",
                        "description": "The parameters for the manual event",
                        "additionalProperties": {
                            "type": "string"
                        }
                    },

                },
                "required": []
            }),
            ui_schema: Some(json!({
                "ui:widget": "object",
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Event,
        }
    }
}

impl ActPackageFn for ManualEventPackage {
    fn start(&self, rt: &Arc<crate::scheduler::Runtime>, options: &Vars) -> Result<Option<Vars>> {
        let mid = options
            .get::<String>(consts::MODEL_ID)
            .ok_or(ActError::Runtime(format!(
                "cannot find '{}' in options",
                consts::MODEL_ID
            )))?;
        let model: ModelInfo = rt.cache().store().models().find(&mid)?.into();
        let workflow = model.workflow()?;
        let params = self.0.clone().unwrap_or_default();
        let ret = rt.start(&workflow, params)?;

        Ok(Some(Vars::new().with(consts::PROCESS_ID, ret.id())))
    }
}

impl<'de> serde::de::Deserialize<'de> for ManualEventPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<Vars>::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<ManualEventPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_event_manual_parse() {
        let params = r#"
            a: 1
            b: abc 
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::ManualEventPackage::meta();
        serde_json::from_value::<super::ManualEventPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }
}
