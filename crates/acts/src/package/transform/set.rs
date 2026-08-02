use crate::package::{ActPackageCatalog, ActPackageDefinition, ActPackageRegister};
use crate::{ActError, ActPackage, Context};
use crate::{Result, Vars, package::ActRunAs};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct SetPackage;

impl ActPackage for SetPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.transform.set",
            name: "Set",
            desc: "set act data from inputs",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-settings2-icon lucide-settings-2"><path d="M14 17H5"/><path d="M19 7h-9"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/></svg>"#,
            doc: "",
            version: "0.1.0",
            schema: json!({
                "type": "object",
                "description": "The key-value pairs to set in the act data",
                "default": {}
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

    fn new(_: &crate::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    fn execute(&self, _ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        let params = serde_json::from_value::<Vars>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;
        // expose the set keys as next inputs
        Ok(Some(params))
    }
}

inventory::submit!(ActPackageRegister::new::<SetPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_set_parse() {
        let params = r#"
            v1: 1
            v2: "string"
            v3:
              - 1
              - 2
              - 3
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::SetPackage::definition();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
