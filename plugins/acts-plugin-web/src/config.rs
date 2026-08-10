use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct HttpConfig {
    pub port: Option<u32>,
}
