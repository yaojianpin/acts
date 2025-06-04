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
pub struct HookEventPackage(Option<Vars>);

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

#[async_trait::async_trait]
impl ActPackageFn for HookEventPackage {
    async fn start(
        &self,
        rt: &Arc<crate::scheduler::Runtime>,
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

        let params = self.0.clone().unwrap_or_default();
        rt.start(&workflow, &params).unwrap();
        let ret = s.recv().await;
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
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
