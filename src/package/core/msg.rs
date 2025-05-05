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
            name: "acts.core.msg",
            desc: "send an message with inputs",
            version: "0.1.0",
            icon: "icon-msg",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Msg,
            group: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

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
}
