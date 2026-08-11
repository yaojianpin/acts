use acts::{ActPackage, ActPackageDefinition, Vars};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Pack2;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pack2Params {
    a: i32,
    b: Vec<String>,
}

#[async_trait::async_trait]
impl ActPackage for Pack2 {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "pack2",
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
            options: Some(json!({
                "ui:order": ["a", "b"]
            })),
            run_as: acts::ActRunAs::Func,
            resources: vec![],
            catalog: acts::ActPackageCatalog::App,
        }
    }

    fn new(_: &acts::Config) -> acts::Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    async fn execute(
        &self,
        _ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> acts::Result<Option<Vars>> {
        println!("execute pack2 with params: {params:?}");
        let params = serde_json::from_value::<Pack2Params>(params.clone()).map_err(|e| {
            acts::ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

        println!("inputs: {params:?}");
        let vars = Vars::new().with("input", params.a + 10);

        Ok(Some(vars))
    }
}
