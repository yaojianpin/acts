use crate::{
    Act, Context, Result, Vars,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
            name: "acts.core.block",
            desc: "run block acts in paralle or sequence",
            version: "0.1.0",
            icon: "icon-block",
            doc: "",
            schema: json!({}),
            run_as: ActRunAs::Func,
            group: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }
}

impl ActPackageFn for BlockPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let mut option = ctx.task().options();
        let mut acts = self.acts.clone();
        for act in acts.iter_mut() {
            // append block options to each child act
            act.options.append(&mut option);
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
