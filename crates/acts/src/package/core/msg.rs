use crate::package::{
    ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MsgPackage;

impl ActPackage for MsgPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.core.msg",
            name: "Message",
            desc: "send an message with inputs",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-send-icon lucide-send"><path d="M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z"/><path d="m21.854 2.147-10.94 10.939"/></svg>"#,
            doc: "",
            schema: json!({
                "type": ["object", "string", "number", "boolean", "array", "null"],
                "title": "Parameters",
                "description": "The message data to send",
                "default": {},
                "additionalProperties": {
                    "type": ["object", "string", "number", "boolean", "array", "null"]
                }
            }),
            options: None,
            run_as: ActRunAs::Msg,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

#[async_trait::async_trait]
impl ActPackageFn for MsgPackage {}

inventory::submit!(ActPackageRegister::new::<MsgPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_msg_parse() {
        let params = r#"
            a: 1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::MsgPackage::meta();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_msg_parse_empty() {
        let value = serde_yaml::from_str::<serde_json::Value>("").unwrap();
        let meta = super::MsgPackage::meta();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_msg_parse_default() {
        let value = serde_yaml::from_str::<serde_json::Value>("{}").unwrap();
        let meta = super::MsgPackage::meta();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_msg_parse_null() {
        let value = serde_json::json!(null);
        let meta = super::MsgPackage::meta();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
