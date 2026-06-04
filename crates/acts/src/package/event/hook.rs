use std::sync::Arc;

use crate::{
    ActError, Channel, ModelInfo, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
    utils::{self, consts},
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct HookEventPackage(Option<Vars>);

impl ActPackage for HookEventPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.event.hook",
            name: "Hook",
            desc: "do an event by hook event",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-webhook-icon lucide-webhook"><path d="M18 16.98h-5.99c-1.1 0-1.95.94-2.48 1.9A4 4 0 0 1 2 17c.01-.7.2-1.4.57-2"/><path d="m6 17 3.13-5.78c.53-.97.1-2.18-.5-3.1a4 4 0 1 1 6.89-4.06"/><path d="m12 6 3.13 5.73C15.66 12.7 16.9 13 18 13a4 4 0 0 1 0 8"/></svg>"#,
            doc: "",
            in_schema: json!({
                "type": "object",
                "properties": {
                    "params": {
                        "type": "object",
                        "title": "Parameters",
                        "description": "The parameters for the hook event",
                        "additionalProperties": {
                            "type": "string"
                        }
                    }
                },
                "required": []
            }),
            ui_schema: Some(json!({
                "ui:widget": "object",
                "ui:options": {
                    "label": false
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Event,
        }
    }
}

impl ActPackageFn for HookEventPackage {
    fn start(&self, rt: &Arc<crate::scheduler::Runtime>, options: &Vars) -> Result<Option<Vars>> {
        let mid = options
            .get::<String>(consts::MODEL_ID)
            .ok_or(ActError::Runtime(format!(
                "cannot find '{}' in options",
                consts::MODEL_ID
            )))?;
        let model: ModelInfo = rt.cache().store().models().find(&mid)?.into();
        let workflow = model.workflow()?;

        let chan = Arc::new(Channel::new(rt));
        let (s, s2, s3) = crate::signal::Signal::new(Vars::new()).triple();
        chan.on_complete(move |m| {
            s2.send(m.outputs.clone());
        });
        chan.on_error(move |m| {
            s3.send(m.outputs.clone());
        });

        let params = self.0.clone().unwrap_or_default();
        rt.start(&workflow, params)?;
        let s_clone = s.clone();
        let ret = utils::sync::block_on(async move { s_clone.recv().await });
        Ok(Some(ret))
    }
}

impl<'de> serde::de::Deserialize<'de> for HookEventPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<Vars>::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<HookEventPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_event_manual_parse() {
        let params = r#"
            p1: 1
            p1: my str
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::HookEventPackage::meta();
        serde_json::from_value::<super::HookEventPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }
}
