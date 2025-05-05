use acts::{ActPackage, ActPackageFn, ActPackageMeta, Context, Vars};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pack1 {
    v1: i32,
}

impl ActPackage for Pack1 {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "pack1",
            desc: "",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: json!({
                "type": "object",
                "properties": {
                    "v1": { "type": "number" }
                }
            }),
            run_as: acts::ActRunAs::Func,
            group: vec![],
            catalog: acts::ActPackageCatalog::App,
        }
    }
}

impl ActPackageFn for Pack1 {
    fn execute(&self, _: &Context) -> acts::Result<Option<Vars>> {
        println!("inputs {:?}", self);
        let mut vars = Vars::new();
        vars.set("input", self.v1);

        Ok(Some(vars))
    }
}
