use crate::{
    ActError, Context, Result, Step, Vars,
    package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs},
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

#[derive(Debug, Clone)]
pub struct BlockPackage;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockPackageParams {
    pub mode: RunningMode,
    pub steps: Vec<Step>,
}

#[async_trait::async_trait]
impl ActPackage for BlockPackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.block",
            name: "Block",
            desc: "run block steps in parallel or sequence",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-grid2x2-icon lucide-grid-2x2"><path d="M12 3v18"/><path d="M3 12h18"/><rect x="3" y="3" width="18" height="18" rx="2"/></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "title": "Mode",
                        "enum": RunningMode::iter().collect::<Vec<_>>(),
                        "default": RunningMode::Sequence.as_ref(),
                        "description": "The running mode of the block"
                    },
                    "steps": {
                        "type": "array",
                        "title": "Steps",
                        "items": { "type": "object" },
                        "description": "The steps to run in the block"
                    }
                },
                "required": ["steps"]
            }),
            options: Some(json!({
                "ui:order": ["mode", "steps"],
                "mode": {
                    "ui:widget": "select",
                    "ui:options": {
                        "placeholder": "Select a mode"
                    }
                },
                "steps": {
                    "ui:widget": "actions",
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn new(_config: &crate::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self)
    }

    async fn execute(&self, _ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        // The kernel fans a block out from the model's own `branches`, which
        // the import materializes from these params; there is nothing left to
        // build at execution time. Parsing stays: it is the act's schema check.
        serde_json::from_value::<BlockPackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

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
        steps:
            - uses: acts.core.msg
              key: msg1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::BlockPackage::definition();
        serde_json::from_value::<super::BlockPackageParams>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_block_parallel_parse() {
        let params = r#"
        mode: parallel
        steps:
            - uses: acts.core.msg
              key: msg1
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::BlockPackage::definition();
        serde_json::from_value::<super::BlockPackageParams>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    /// An act the block params cannot deserialize is the package's input
    /// error, reported with its package id.
    #[tokio::test]
    async fn pack_block_execute_rejects_malformed_acts() {
        let ctx = crate::package::tests::completed_context().await;
        let err = super::BlockPackage
            .execute(&ctx, &serde_json::json!({"steps": [{"id": 1}]}))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid ActPackage(acts.core.block)"),
            "got: {err}"
        );
    }
}
