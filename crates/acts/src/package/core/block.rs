use crate::{
    Act, Context, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strum::IntoEnumIterator;

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    strum::AsRefStr,
    strum::EnumString,
    strum::EnumIter,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RunningMode {
    #[default]
    Sequence,
    Parallel,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockPackage {
    pub mode: RunningMode,
    pub acts: Vec<Act>,
}

impl ActPackage for BlockPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.core.block",
            name: "Block",
            desc: "run block acts in paralle or sequence",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-grid2x2-icon lucide-grid-2x2"><path d="M12 3v18"/><path d="M3 12h18"/><rect x="3" y="3" width="18" height="18" rx="2"/></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": RunningMode::iter().collect::<Vec<_>>(),
                        "default": RunningMode::Sequence.as_ref(),
                        "description": "The running mode of the block"
                    },
                    "acts": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "The acts to run in the block"
                    }
                },
                "required": ["acts"]
            }),
            options: Some(json!({
                "ui:order": ["mode", "acts"],
                "mode": {
                    "ui:widget": "select",
                    "ui:options": {
                        "label": false,
                        "placeholder": "Select a mode"
                    }
                },
                "acts": {
                    "ui:widget": "acts",
                    "ui:options": {
                        "label": false
                    }
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl ActPackageFn for BlockPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let mut options = ctx.task().options();
        let mut acts = self.acts.clone();
        for act in acts.iter_mut() {
            // append block options to each child act
            act.options.append(&mut options);
        }
        ctx.build_acts(&acts, self.mode == RunningMode::Sequence)?;
        Ok(None)
    }
}

inventory::submit!(ActPackageRegister::new::<BlockPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;

    #[test]
    fn pack_block_sequence_parse() {
        let params = r#"
        mode: sequence
        acts:
            - uses: acts.core.msg
              key: msg1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::BlockPackage::meta();
        serde_json::from_value::<super::BlockPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_block_parallel_parse() {
        let params = r#"
        mode: parallel
        acts:
            - uses: acts.core.msg
              key: msg1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::BlockPackage::meta();
        serde_json::from_value::<super::BlockPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
