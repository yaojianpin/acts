use crate::{
    ActError, Action, Config, Context, Result, Vars,
    event::EventAction,
    package::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActPackageRegister, ActRunAs},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strum::IntoEnumIterator;

#[derive(Debug, Clone)]
pub struct ActionPackage;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ActionPackageParams {
    action: EventAction,
    options: Option<Vars>,
}

impl ActPackage for ActionPackage {
    fn new(_config: &Config) -> Result<Self> {
        Ok(Self)
    }
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.core.action",
            name: "Action",
            desc: "do an action with inputs",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-check-icon lucide-check"><path d="M20 6 9 17l-5-5"/></svg>"#,
            doc: "",
            schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "title": "Action",
                        "enum": EventAction::iter().collect::<Vec<_>>(),
                        "default": EventAction::Next.as_ref(),
                        "description": "The action to execute"
                    },
                    "options": {
                        "type": ["object", "null"],
                        "title": "Options",
                        "description": "Additional options for the action"
                    }
                },
                "required": ["action"]
            }),
            options: Some(json!({
                "ui:order": ["action", "options"],
                "action": {
                    "ui:widget": "select",
                },
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Core,
        }
    }

    fn execute(&self, ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        let task = ctx.task();

        let params =
            serde_json::from_value::<ActionPackageParams>(params.clone()).map_err(|e| {
                ActError::Package(format!(
                    "invalid ActPackage({}) params: {}",
                    Self::definition().id,
                    e
                ))
            })?;

        ctx.set_action(&Action::new(
            &task.pid,
            &task.id,
            params.action.clone(),
            params.options.unwrap_or_default().clone(),
        ))?;
        task.update(ctx)?;
        Ok(None)
    }
}

inventory::submit!(ActPackageRegister::new::<ActionPackage>());

#[cfg(test)]
mod tests {
    use crate::ActPackage;
    #[cfg(test)]
    use crate::Vars;

    #[test]
    fn pack_action_parse() {
        let actions = vec![
            "next", "submit", "back", "cancel", "abort", "skip", "error", "push", "remove",
        ];

        for action in actions {
            pack_action(action, Vars::new().with("a", 1))
        }
    }

    #[test]
    fn pack_action_with_option_default() {
        let actions = vec![
            "next", "submit", "back", "cancel", "abort", "skip", "error", "push", "remove",
        ];

        for action in actions {
            pack_action(action, Vars::default())
        }
    }

    #[test]
    fn pack_action_without_option() {
        let actions = vec![
            "next", "submit", "back", "cancel", "abort", "skip", "error", "push", "remove",
        ];

        for action in actions {
            pack_action_without_options(action)
        }
    }

    #[cfg(test)]
    fn pack_action(action: &str, options: Vars) {
        use serde_json::json;

        let params = json!({
            "action": action,
            "options": options,
        });

        let meta = super::ActionPackage::definition();
        serde_json::from_value::<super::ActionPackageParams>(params.clone()).unwrap();
        jsonschema::validate(&meta.schema, &params).unwrap()
    }

    #[cfg(test)]
    fn pack_action_without_options(action: &str) {
        use serde_json::json;

        let params = json!({
            "action": action,
        });

        let meta = super::ActionPackage::definition();
        serde_json::from_value::<super::ActionPackageParams>(params.clone()).unwrap();
        jsonschema::validate(&meta.schema, &params).unwrap()
    }
}
