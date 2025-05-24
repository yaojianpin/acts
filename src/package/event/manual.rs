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
pub struct ManualEventPackage(serde_json::Value);

impl ActPackage for ManualEventPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.event.manual",
            desc: "do an event by manual",
            version: "0.1.0",
            icon: "icon-manual",
            doc: "",
            schema: json!({}),
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
        let options = options.clone().with(consts::ACT_VALUE, self.0.clone());
        let ret = rt.start(&workflow, &options)?;

        Ok(Some(Vars::new().with(consts::PROCESS_ID, ret.id())))
    }
}

impl<'de> serde::de::Deserialize<'de> for ManualEventPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
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
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
