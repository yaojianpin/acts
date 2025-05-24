use std::sync::Arc;

use crate::{
    ActError, Channel, ModelInfo, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
    utils::consts,
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct HookEventPackage(Vars);

impl ActPackage for HookEventPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.event.hook",
            desc: "do an event by hook event",
            version: "0.1.0",
            icon: "icon-hook",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Event,
        }
    }
}

impl ActPackageFn for HookEventPackage {
    fn start(&self, rt: &Arc<crate::scheduler::Runtime>, options: &Vars) -> Result<Option<Vars>> {
        let chan = Arc::new(Channel::new(rt));

        let mid = options
            .get::<String>(consts::MODEL_ID)
            .ok_or(ActError::Runtime(format!(
                "cannot find '{}' in options",
                consts::MODEL_ID
            )))?;
        let model: ModelInfo = rt.cache().store().models().find(&mid)?.into();
        let workflow = model.workflow()?;
        rt.start(&workflow, options)?;

        let (tx, rx) = std::sync::mpsc::channel();
        chan.on_complete(move |m| tx.send(m.outputs.clone()).unwrap());

        let ret = rx
            .recv()
            .map_err(|e| ActError::Runtime(format!("failed to receive process outputs: {}", e)))?;
        Ok(Some(ret))
    }
}

impl<'de> serde::de::Deserialize<'de> for HookEventPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Vars::deserialize(deserializer)?;
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
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
