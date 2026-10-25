use crate::package::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, Result,
};
use crate::{ActError, Context, Step, Vars, package::ActRunAs};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone)]
pub struct SequencePackage;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SequencePackageParams {
    r#in: Vec<JsonValue>,
    steps: Vec<Step>,
}

#[async_trait::async_trait]
impl ActPackage for SequencePackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.sequence",
            name: "Sequence",
            desc: "generate sequence steps and run them in sequence",
            version: "0.1.0",
            icon: r#"<svg viewBox="0 0 1024 1024" version="1.1" xmlns="http://www.w3.org/2000/svg" width="24" height="24" fill="currentColor"><path d="M920 760H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM920 192H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM920 476H336c-4.4 0-8 3.6-8 8v56c0 4.4 3.6 8 8 8h584c4.4 0 8-3.6 8-8v-56c0-4.4-3.6-8-8-8zM216 712H100c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h72.4v20.5h-35.7c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h35.7V838H100c-2.2 0-4 1.8-4 4v34c0 2.2 1.8 4 4 4h116c2.2 0 4-1.8 4-4V716c0-2.2-1.8-4-4-4zM100 188h38v120c0 2.2 1.8 4 4 4h40c2.2 0 4-1.8 4-4V152c0-4.4-3.6-8-8-8h-78c-2.2 0-4 1.8-4 4v36c0 2.2 1.8 4 4 4zM216 428H100c-2.2 0-4 1.8-4 4v36c0 2.2 1.8 4 4 4h68.4l-70.3 77.7c-1.3 1.5-2.1 3.4-2.1 5.4V592c0 2.2 1.8 4 4 4h116c2.2 0 4-1.8 4-4v-36c0-2.2-1.8-4-4-4h-68.4l70.3-77.7c1.3-1.5 2.1-3.4 2.1-5.4V432c0-2.2-1.8-4-4-4z"></path></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "in": {
                        "type": "array",
                        "title": "In",
                        "description": "The input array to create steps from",
                        "items": {
                            "type": "string"
                         }
                    },
                    "steps": {
                        "type": "array",
                        "title": "Actions",
                        "description": "The steps to run in sequence",
                        "items": {
                            "type": "object"
                         },
                    }
                },
                "required": ["in", "steps"]
            }),
            options: Some(json!({
                 "ui:order": ["in", "steps"],
                "in": {
                    "ui:widget": "in",
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

    fn new(_config: &crate::Config) -> crate::Result<Self> {
        Ok(Self)
    }

    async fn execute(&self, _ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        // The kernel fans the sequence out from the model's own `branches`,
        // which the import materializes from these params; there is nothing
        // left to build at execution time. Parsing stays: it is the act's
        // schema check.
        serde_json::from_value::<SequencePackageParams>(params.clone()).map_err(|e| {
            ActError::Package(format!(
                "invalid ActPackage({}) params: {}",
                Self::definition().id,
                e
            ))
        })?;

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
        steps:
            - uses: acts.core.irq
              params:
                a: 1
            - uses: acts.core.irq
              params:
                b: 10
    "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::SequencePackage::definition();
        serde_json::from_value::<super::SequencePackageParams>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    /// An entry the sequence params cannot deserialize is the package's input
    /// error, reported with its package id.
    #[tokio::test]
    async fn pack_sequence_execute_rejects_malformed_steps() {
        let ctx = crate::package::tests::completed_context().await;
        let err = super::SequencePackage
            .execute(&ctx, &serde_json::json!({"in": [], "steps": [{"id": 1}]}))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid ActPackage(acts.core.sequence)"),
            "got: {err}"
        );
    }
}
