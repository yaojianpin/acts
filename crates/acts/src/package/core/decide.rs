use crate::package::{
    ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs,
};
use serde_json::json;

/// The join-decision request a container with `rule: { action, params }`
/// sends its client.
///
/// The engine creates one request execution per child settlement (the
/// delegated rule asks on every vote that lands, so the client can decide as
/// early as it can) and waits for the client's answer:
///
/// - `complete` — the container completes;
/// - `fail` — the container fails with the answer's error;
/// - `skip` — defer: the request settles skipped and the next settlement (or
///   an idle re-ask once every child settled) asks again.
///
/// The request's params carry the `action` key, the rule's own `params`, and
/// the `outcomes` census: every counted child with its node, generation,
/// state and outputs. A dedicated package (rather than `acts.core.irq`)
/// constrains that contract and lets a client route the request by its
/// `uses`; the transport underneath is the same interrupt-and-answer flow.
#[derive(Debug, Clone)]
pub struct JoinDecidePackage;

#[async_trait::async_trait]
impl ActPackage for JoinDecidePackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.decide",
            name: "Rule",
            desc: "ask the client to decide a container's join rule",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-scale-icon lucide-scale"><path d="m16 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/><path d="m2 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/><path d="M7 21h10"/><path d="M12 3v18"/><path d="M3 7h2c2 0 5-1 7-2 2 1 5 2 7 2h2"/></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "title": "Parameters",
                "description": "The join-decision request: the action to invoke, the rule's params, and the children's outcomes",
                "required": ["action"],
                "properties": {
                    "action": {
                        "type": "string",
                        "title": "Action",
                        "description": "The key the client's handler is registered under"
                    },
                    "params": {
                        "type": ["object", "null"],
                        "title": "Params",
                        "description": "The rule's own params, resolved against the container scope",
                        "default": {}
                    },
                    "outcomes": {
                        "type": "array",
                        "title": "Outcomes",
                        "description": "One entry per counted child: {node, generation, state, outputs}",
                        "items": { "type": "object" }
                    }
                }
            }),
            options: None,
            run_as: ActRunAs::Irq,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn new(_config: &crate::Config) -> crate::Result<Self> {
        Ok(Self)
    }
}

inventory::submit!(ActPackageRegister::new::<JoinDecidePackage>());

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::ActPackage;

    #[test]
    fn pack_rule_parse() {
        let params = r#"
            action: countersign
            params:
              chair: chair
            outcomes:
              - node: "s1#act0"
                generation: 0
                state: completed
                outputs: {}
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = super::JoinDecidePackage::definition();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }

    #[test]
    fn pack_rule_requires_the_action() {
        let value = serde_yaml::from_str::<serde_json::Value>("params: {}").unwrap();
        let meta = super::JoinDecidePackage::definition();
        assert!(jsonschema::validate(&meta.schema, &value).is_err());
    }

    #[test]
    fn pack_rule_new() {
        let _package = super::JoinDecidePackage::new(&crate::Config::default()).unwrap();
        // the request waits for the client's verdict: it is not a function package
        assert_eq!(
            super::JoinDecidePackage::definition().run_as,
            crate::ActRunAs::Irq
        );
    }
}
