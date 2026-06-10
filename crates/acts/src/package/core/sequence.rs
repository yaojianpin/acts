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
            id: "acts.core.sequence",
            name: "Sequence",
            desc: "generate sequence acts and run them in sequence",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M920 760H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM920 192H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM920 476H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM216 712H100c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h72.4v20.5h-35.7c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h35.7V838H100c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h116c2.2 0 4-1.8 4-4V716c0-2.2-1.8-4-4-4zM100 188h38v120c0 2.2 1.8 4 4 4h40c2.2 0 4-1.8 4-4V152c0-4.4-3.6-8-8-8h-78c-2.2 0-4 1.8-4 4v36c0 2.2 1.8 4 4 4zM216 428H100c-2.2 0-4 1.8-4 4v36c0 2.2 1.8 4 4 4h68.4l-70.3 77.7c-1.3 1.5-2.1 3.4-2.1 5.4V592c0 2.2 1.8 4 4 4h116c2.2 0 4-1.8 4-4v-36c0-2.2-1.8-4-4-4h-68.4l70.3-77.7c1.3-1.5 2.1-3.4 2.1-5.4V432c0-2.2-1.8-4-4-4z"></path></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "in": {
                        "type": "array",
                        "title": "In",
                        "description": "The input array to create acts from",
                        "items": {
                            "type": "string"
                         }
                    },
                    "acts": {
                        "type": "array",
                        "title": "Actions",
                        "description": "The actions to run in sequence",
                        "items": {
                            "type": "object"
                         },
                    }
                },
                "required": ["in", "acts"]
            }),
            options: Some(json!({
                "acts": {
                    "ui:widget": "actions",
                }
            })),
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
