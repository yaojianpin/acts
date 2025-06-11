use crate::ActUserVar;

/// use secrets var to read the secrets data from task context
#[derive(Clone)]
pub struct SecretsVar;

impl ActUserVar for SecretsVar {
    fn name(&self) -> String {
        "secrets".to_string()
    }
}
