use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DbConfig {
    pub database_url: String,
}
