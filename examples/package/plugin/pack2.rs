use acts::{ActPackage, ActPackageFn, ActPackageMeta, Context, Vars};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pack2 {
    a: i32,
    b: Vec<String>,
}

impl ActPackage for Pack2 {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "pack2",
            desc: "",
            icon: "",
            doc: "",
            version: "0.1.0",
            schema: json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "array" }
                }
            }),
            run_as: acts::ActRunAs::Func,
            group: vec![],
            catalog: acts::ActPackageCatalog::App,
        }
    }
}

impl ActPackageFn for Pack2 {
    fn execute(&self, _ctx: &Context) -> acts::Result<Option<Vars>> {
        println!("inputs: {:?}", self);
        let vars = Vars::new().with("input", self.a + 10);

        Ok(Some(vars))
    }
}
