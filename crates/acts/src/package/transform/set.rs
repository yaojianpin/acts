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
            id: "acts.transform.set",
            name: "Set",
            desc: "set act data from inputs",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-settings2-icon lucide-settings-2"><path d="M14 17H5"/><path d="M19 7h-9"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/></svg>"#,
            doc: "",
            version: "0.1.0",
            schema: json!({
                "type": "object",
                "description": "The key-value pairs to set in the act data",
                "default": {},
                "additionalProperties": {
                    "type": ["string", "number", "array", "boolean", "null"]
                }
            }),
            options: Some(json!({
                "ui:options": {
                    "expandable": true,
                    "addable": true,
                    "orderable": true,
                    "removable": true
                }
            })),
            run_as: ActRunAs::Func,
            resources: Vec::new(),
            catalog: ActPackageCatalog::Transform,
        }
    }
}

impl ActPackageFn for SetPackage {
    fn execute(&self, _ctx: &Context) -> Result<Option<Vars>> {
        // expose the set keys as next inputs
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
