use acts::{ActPackage, ActPackageFn, ActPackageMeta};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Deserialize, Serialize)]
pub struct Params {
    pub a: i32,
    pub b: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Pack3 {
    pub func: String,
    pub options: Params,
}

impl ActPackage for Pack3 {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "pack3",
            desc: "",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: json!({
                "type": "object",
                "properties": {
                    "func": { "type": "string" },
                    "options": {
                        "type": "object",
                        "properties": {
                            "a": { "type": "number" },
                            "b": { "type": "string" }
                        }
                    }
                }
            }),
            run_as: acts::ActRunAs::Irq,
            group: vec![],
            catalog: acts::ActPackageCatalog::App,
        }
    }
}

impl ActPackageFn for Pack3 {}
