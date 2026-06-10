use crate::{
    Action, Context, Result, Vars,
    event::EventAction,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strum::IntoEnumIterator;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionPackage {
    pub action: EventAction,

    #[serde(default)]
    pub options: Vars,
}

impl ActPackage for ActionPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
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
                    "ui:options": {
                        "label": false,
                        "placeholder": "Select an action"
                    }
                },
                "options": {
                    "ui:widget": "object",
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

impl ActPackageFn for ActionPackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        let task = ctx.task();
        if let Some(parent) = task.parent() {
            ctx.set_task(&parent);
            ctx.set_action(&Action::new(
                &parent.pid,
                &parent.id,
                self.action.clone(),
                self.options.clone(),
            ))?;
            parent.update(ctx)?;
        }
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
            pack_action_witout_options(action)
        }
    }

    #[cfg(test)]
    fn pack_action(action: &str, options: Vars) {
        use serde_json::json;

        let params = json!({
            "action": action,
            "options": options,
        });

        let meta = super::ActionPackage::meta();
        serde_json::from_value::<super::ActionPackage>(params.clone()).unwrap();
        jsonschema::validate(&meta.schema, &params).unwrap()
    }

    #[cfg(test)]
    fn pack_action_witout_options(action: &str) {
        use serde_json::json;

        let params = json!({
            "action": action,
        });

        let meta = super::ActionPackage::meta();
        serde_json::from_value::<super::ActionPackage>(params.clone()).unwrap();
        jsonschema::validate(&meta.schema, &params).unwrap()
    }
}
