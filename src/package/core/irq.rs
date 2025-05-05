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
            name: "acts.core.irq",
            desc: "send an interrupt request to client with inputs",
            version: "0.1.0",
            icon: "icon-irq",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Irq,
            group: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

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
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
