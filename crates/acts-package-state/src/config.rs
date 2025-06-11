#[derive(serde::Deserialize)]
pub struct StateConfig {
    // redis uri
    pub database_uri: String,
}
