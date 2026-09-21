use crate::env::Environment;
use acts::{ActPackage, ActPackageCatalog, ActPackageDefinition, ActRunAs, Context, Result, Vars};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct CodePackage {
    env: Environment,
}

#[async_trait::async_trait]
impl ActPackage for CodePackage {
    fn definition() -> ActPackageDefinition {
        ActPackageDefinition {
            id: "acts.transform.code.javascript",
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

    fn new(_: &acts::Config) -> Result<Self>
    where
        Self: Sized,
    {
        Ok(Self {
            env: Environment::new(),
        })
    }

    async fn execute(&self, _ctx: &Context, params: &serde_json::Value) -> Result<Option<Vars>> {
        let Some(code) = params.as_str() else {
            return Err(acts::ActError::Package(
                "Code package requires a string parameter".to_string(),
            ));
        };

        // wrap the code into a function to support return syntax
        let code_fn = format!(r#"(()=>{{ {} }})()"#, code);
        let outputs = self.env.eval::<serde_json::Value>(&code_fn)?;
        let mut ret = None;
        if let serde_json::Value::Object(map) = outputs {
            ret = Some(Vars::from(map));
        }
        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use crate::CodePackage;
    use acts::ActPackage;

    #[test]
    fn pack_code_parse() {
        let params = r#"
            console.log("Hello, World!");
        "#;

        let value = serde_yaml::from_str::<serde_json::Value>(params).unwrap();
        let meta = CodePackage::definition();
        jsonschema::validate(&meta.schema, &value).unwrap()
    }
}
