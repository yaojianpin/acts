use crate::package::{
    ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IrqPackage;

impl ActPackage for IrqPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.core.irq",
            name: "Request",
            desc: "send a request to client with params",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M876 529.4c-15.9 0-28 12.1-28 28v249.2c0 38.3-34.5 69.1-76.5 69.1h-575c-42 0-76.5-30.8-76.5-69.1V272.8c0-38.3 34.5-69.1 76.5-69.1h270.7c15.9 0 28-12.1 28-28s-12.1-28-28-28H196.5c-72.8 0-132.5 56-132.5 125.1v533.9c0 69.1 59.7 125.1 132.5 125.1h574.9c72.8 0 132.5-56 132.5-125.1V557.4c0.1-15.8-12-28-27.9-28z m0 0" p-id="13377"></path><path d="M932 91.7H642.7c-15.9 0-28 12.1-28 28s12.1 28 28 28h222.1L389.7 622.8c-9.3 10.3-9.3 27.1 0 38.3 5.6 5.6 13.1 9.3 20.5 9.3 6.5 0 13.1-2.8 18.7-7.5l475.1-476V409c0 15.9 12.1 28 28 28s28-12.1 28-28V119.7c0-15.9-12.1-28-28-28z m0 0"></path></svg>"#,
            doc: "",
            in_schema: json!({
                "type": ["object", "string", "number", "boolean", "array", "null"],
                "title": "Parameters",
                "description": "The interrupt request data to send",
                "default": {},
                "additionalProperties": {
                    "type": ["object", "string", "number", "boolean", "array", "null"]
                }
            }),
            ui_schema: None,
            run_as: ActRunAs::Irq,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

#[async_trait::async_trait]
impl ActPackageFn for IrqPackage {}

inventory::submit!(ActPackageRegister::new::<IrqPackage>());

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::ActPackage;

    #[test]
    fn pack_irq_parse() {
        let params = r#"
            a: 1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::IrqPackage::meta();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }

    #[test]
    fn pack_irq_parse_empty() {
        let value = serde_yaml::from_str::<serde_json::Value>("").unwrap();
        let meta = super::IrqPackage::meta();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }

    #[test]
    fn pack_irq_parse_default() {
        let value = serde_yaml::from_str::<serde_json::Value>("{}").unwrap();
        let meta = super::IrqPackage::meta();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }

    #[test]
    fn pack_irq_parse_null() {
        let value = serde_json::json!(null);
        let meta = super::IrqPackage::meta();
        jsonschema::validate(&meta.in_schema, &value).unwrap()
    }
}
