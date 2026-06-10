use crate::package::{
    ActPackageCatalog, ActPackageFn, ActPackageMeta, ActPackageRegister, ActRunAs,
};
use crate::{ActPackage, Context, Result, Vars};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct CodePackage(String);

impl ActPackage for CodePackage {
    fn meta() -> ActPackageMeta {
        ActPackageMeta {
            id: "acts.transform.code",
            name: "Code",
            desc: "run javascript code",
            version: "0.1.0",
            icon: r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-braces-icon lucide-braces"><path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1"/><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1"/></svg>"#,
            doc: "",
            schema: json!({
                "type": "string",
                "description": "The JavaScript code to execute",
                "default": "",
            }),
            options: Some(json!({
                "ui:widget": "textarea",
                "ui:options": {
                    "placeholder": "Type your JavaScript code here..."
                }
            })),
            run_as: ActRunAs::Func,
            resources: vec![],
            catalog: ActPackageCatalog::Transform,
        }
    }
}

impl ActPackageFn for CodePackage {
    fn execute(&self, ctx: &Context) -> Result<Option<Vars>> {
        // wrap the code into a function to support return synax
        let code = format!(r#"(()=>{{ {} }})()"#, self.0);
        let outputs = ctx.eval::<serde_json::Value>(&code)?;
        let mut ret = None;
        if let serde_json::Value::Object(map) = outputs {
            ret = Some(Vars::from(map));
        }
        Ok(ret)
    }
}

impl<'de> serde::de::Deserialize<'de> for CodePackage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self(value))
    }
}

inventory::submit!(ActPackageRegister::new::<CodePackage>());
