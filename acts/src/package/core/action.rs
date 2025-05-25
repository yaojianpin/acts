use crate::{
    Action, Context, Result, Vars,
    event::EventAction,
    package::{
        ActPackage, ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionPackage {
    pub action: EventAction,

    #[serde(default)]
    pub options: Vars,
}

impl ActPackage for ActionPackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            name: "acts.core.action",
            desc: "do an action with inputs",
            version: "0.1.0",
            icon: "icon-action",
            doc: "",
            schema: json!({}),
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
                &self.options,
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

    #[test]
    fn pack_action_parse() {
        let actions = vec![
            "next", "submit", "back", "cancel", "abort", "skip", "error", "push", "remove",
        ];

        for action in actions {
            pack_action(action)
        }
    }

    #[cfg(test)]
    fn pack_action(action: &str) {
        let params = format!(
            r#"
        action: {action}
        options:
            a: 1
        "#
        );

        let value = serde_yaml::from_str::<serde_json::Value>(&params).unwrap();
        let meta = super::ActionPackage::meta();
        serde_json::from_value::<super::ActionPackage>(value.clone()).unwrap();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
