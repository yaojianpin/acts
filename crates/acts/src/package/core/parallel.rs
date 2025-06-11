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
            name: "acts.core.parallel",
            desc: "create acts based on an array and run them in parallel",
            version: "0.1.0",
            icon: "icon-parallel",
            doc: "",
            schema: json!({}),
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
                mode: RunningMode::Sequence,
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
