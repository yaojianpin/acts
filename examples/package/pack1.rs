use acts::{ActPackage, ActPackageDefinition, Vars};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Pack1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pack1Params {
    v1: i32,
}

impl ActPackage for Pack1 {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "pack1",
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
            options: None,
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

    fn execute(
        &self,
        _ctx: &acts::Context,
        params: &serde_json::Value,
    ) -> acts::Result<Option<Vars>> {
        println!("execute pack1 with params: {params:?}");
        let params = serde_json::from_value::<Pack1Params>(params.clone()).map_err(|e| {
            acts::ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;
        println!("inputs {params:?}");
        let mut vars = Vars::new();
        vars.set("input", params.v1);

        Ok(Some(vars))
    }
}
