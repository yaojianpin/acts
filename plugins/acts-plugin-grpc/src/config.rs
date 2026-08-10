use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct GrpcConfig {
    pub port: Option<u32>,
}
