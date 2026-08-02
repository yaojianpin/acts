use std::sync::Arc;

use crate::{
    ActError, Channel, ModelInfo, Result, Vars,
    package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs},
    utils::{self, consts},
};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct HookEventPackage;

impl ActPackage for HookEventPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.event.hook",
            name: "Hook",
            desc: "do an event by hook event",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-webhook-icon lucide-webhook"><path d="M18 16.98h-5.99c-1.1 0-1.95.94-2.48 1.9A4 4 0 0 1 2 17c.01-.7.2-1.4.57-2"/><path d="m6 17 3.13-5.78c.53-.97.1-2.18-.5-3.1a4 4 0 1 1 6.89-4.06"/><path d="m12 6 3.13 5.73C15.66 12.7 16.9 13 18 13a4 4 0 0 1 0 8"/></svg>"#,
            doc: "",
            schema: json!({
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
            options: None,
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Event,
        }
    }

    fn new(_: &crate::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    fn start(
        &self,
        rt: &Arc<crate::scheduler::Runtime>,
        params: &serde_json::Value,
        options: &Vars,
    ) -> Result<Option<Vars>> {
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

        let params = serde_json::from_value::<Vars>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;
        rt.start(&workflow, params)?;
        let s_clone = s.clone();
        let ret = utils::sync::block_on(async move { s_clone.recv().await });
        Ok(Some(ret))
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
        let meta = super::HookEventPackage::definition();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
