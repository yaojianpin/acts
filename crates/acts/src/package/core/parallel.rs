use super::super::core::{BlockPackage, RunningMode};
use crate::{
    Act, Context, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
    utils::consts,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParallelPackage {
    r#in: Vec<JsonValue>,
    acts: Vec<Act>,
}

impl ActPackage for ParallelPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.core.parallel",
            name: "Parallel",
            desc: "create acts based on an array and run them in parallel",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M822.656 649.344l128 128v45.312l-128 128-45.312-45.312L850.752 832H250.496a96 96 0 1 1 0-64h600.256l-73.408-73.344z m0-640l-45.312 45.312L850.752 128H250.496a96 96 0 1 0 0 64h600.256l-73.408 73.344 45.312 45.312 128-128v-45.312z m-45.312 365.312L850.752 448H250.496a96 96 0 1 0 0 64h600.256l-73.408 73.344 45.312 45.312 128-128v-45.312l-128-128z" fill="currentColor"></path></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "in": {
                        "type": "array",
                        "title": "In",
                        "items": { "type": "string" },
                        "description": "The input array to create acts"
                    },
                    "acts": {
                        "type": "array",
                        "title": "Act List",
                        "items": { "type": "object" },
                        "description": "The acts to run in parallel"
                    }
                },
                "required": ["in", "acts"]
            }),
            options: Some(json!({
                "in": {
                    "ui:widget": "array",
                    "ui:options": {
                        "label": false,
                        "addButtonText": "Add Input"
                    }
                },
                "acts": {
                    "ui:widget": "acts",
                    "ui:options": {
                        "label": false,
                        "addButtonText": "Add Act"
                    }
                }
            })),

            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl ActPackageFn for ParallelPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let mut acts = Vec::new();
        for (index, value) in self.r#in.iter().enumerate() {
            let params = serde_json::to_value(BlockPackage {
                mode: RunningMode::Parallel,
                acts: self.acts.clone(),
            })?;

            acts.push(Act {
                uses: "acts.core.block".to_string(),
                options: Vars::new()
                    .with(consts::ACT_INDEX, index)
                    .with(consts::ACT_VALUE, value),
                params,
                ..Default::default()
            });
        }
        ctx.build_acts(&acts, false)?;
        Ok(None)
    }
}

inventory::submit!(ActPackageRegister::new::<ParallelPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_parallel_parse() {
        let params = r#"
            in: ["u1", "u2"]
            acts:
                - uses: acts.core.irq
                  params:
                    a: 1
                - uses: acts.core.irq
                  params:
                    b: 10
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::ParallelPackage::meta();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
