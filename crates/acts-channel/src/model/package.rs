use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub icon: String,
    pub doc: String,
    pub version: String,
    pub schema: serde_json::Value,
    pub options: Option<serde_json::Value>,
    pub run_as: String,
    pub resources: Vec<serde_json::Value>,
    pub catalog: String,
}

#[cfg(test)]
mod tests {
    use super::Package;
    use serde_json::json;

    #[test]
    fn serde_roundtrip() {
        let text = r#"{
            "id": "pkg",
            "name": "package",
            "desc": "a package",
            "icon": "icon.svg",
            "doc": "docs",
            "version": "1.0",
            "schema": {"params": {"a": "string"}},
            "options": {"tag": "x"},
            "run_as": "user",
            "resources": [{"id": "r1"}],
            "catalog": "app"
        }"#;
        let package: Package = serde_json::from_str(text).unwrap();
        assert_eq!(package.id, "pkg");
        assert_eq!(package.schema["params"]["a"], json!("string"));
        assert_eq!(package.options.as_ref().unwrap()["tag"], json!("x"));
        assert_eq!(package.resources.len(), 1);

        let value = serde_json::to_value(&package).unwrap();
        assert_eq!(value["id"], json!("pkg"));
        assert_eq!(value["resources"][0]["id"], json!("r1"));

        // `options` is optional
        let no_options = r#"{
            "id": "pkg",
            "name": "package",
            "desc": "",
            "icon": "",
            "doc": "",
            "version": "1.0",
            "schema": {},
            "run_as": "",
            "resources": [],
            "catalog": ""
        }"#;
        let package: Package = serde_json::from_str(no_options).unwrap();
        assert!(package.options.is_none());
    }
}
