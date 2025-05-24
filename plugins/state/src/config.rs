#[derive(serde::Deserialize)]
pub struct StateConfig {
    // redis uri
    pub uri: String,
}
