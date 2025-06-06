use super::super::core::{BlockPackage, RunningMode};
use crate::Context;
use crate::package::{ActPackage, ActPackageCatalog, ActPackageMeta, ActPackageRegister, Result};
use crate::utils::consts;
use crate::{
    Act, Vars,
    package::{ActPackageFn, ActRunAs},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SequencePackage {
    r#in: Vec<JsonValue>,
    acts: Vec<Act>,
}

impl ActPackage for SequencePackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.core.sequence",
            desc: "generate sequence acts and run them in sequence",
            version: "0.1.0",
            icon: "icon-sequence",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl ActPackageFn for SequencePackage {
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
            })
        }

        ctx.build_acts(&acts, true)?;
        Ok(None)
    }
}

inventory::submit!(ActPackageRegister::new::<SequencePackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_sequence_parse() {
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
        let meta = super::SequencePackage::meta();
        serde_json::from_value::<super::SequencePackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
