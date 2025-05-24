use crate::package::{
    ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
};
use crate::{ActPackage, Context, Result, Vars};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct CodePackage(String);

impl ActPackage for CodePackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.transform.code",
            desc: "run javascript code",
            version: "0.1.0",
            icon: "icon-code",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Transform,
        }
    }
}

impl ActPackageFn for CodePackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let outputs = ctx.eval::<serde_json::Value>(&self.0)?;
        let mut ret = None;
        if let serde_json::Value::Object(map) = outputs {
            ret = Some(Vars::from(map));
        }
        Ok(ret)
    }
}

impl<'de> serde::de::Deserialize<'de> for CodePackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<CodePackage>());
