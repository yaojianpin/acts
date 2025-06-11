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
pub struct ManualEventPackage(Option<String>);

impl ActPackage for ManualEventPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.event.chat",
            desc: "do an event by chat",
            version: "0.1.0",
            icon: "icon-chat",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Event,
        }
    }
}

#[async_trait::async_trait]
impl ActPackageFn for ManualEventPackage {
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

        let mut params = Vars::new();
        if let Some(ref v) = self.0 {
            params.insert(consts::ACT_DATA.to_string(), json!(v));
        }
        let ret = rt.start(&workflow, &params)?;

        Ok(Some(Vars::new().with(consts::PROCESS_ID, ret.id())))
    }
}

impl<'de> serde::de::Deserialize<'de> for ManualEventPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<ManualEventPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_event_manual_parse() {
        let params = r#"hello world"#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::ManualEventPackage::meta();
        serde_json::from_value::<super::ManualEventPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
