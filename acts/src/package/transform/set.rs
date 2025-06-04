use crate::package::{ActPackageCatalog, ActPackageMeta, ActPackageRegister};
use crate::{ActPackage, Context};
use crate::{
    Result, Vars,
    package::{ActPackageFn, ActRunAs},
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct SetPackage(Vars);

impl ActPackage for SetPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.transform.set",
            desc: "set act data from inputs",
            icon: "icon-set",
            doc: "",
            version: "0.1.0",
            schema: json!({}),
            run_as: ActRunAs::Func,
            resources: Vec::new(),
            catalog: ActPackageCatalog::Transform,
        }
    }
}

impl ActPackageFn for SetPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        // expose the set keys as next inputs
        let keys = self.0.keys().map(|k| k.as_str()).collect::<Vec<_>>();
        ctx.task().expose(&keys);
        Ok(Some(self.0.clone()))
    }
}

impl<'de> serde::de::Deserialize<'de> for SetPackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Vars::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<SetPackage>());
