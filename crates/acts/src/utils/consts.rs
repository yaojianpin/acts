pub const ACT_USE_PARENT_PROC_ID: &str = "$parent_pid";
pub const ACT_USE_PARENT_TASK_ID: &str = "$parent_tid";

pub const STEP_NODE_ID: &str = "node_id";
pub const STEP_NODE_NAME: &str = "node_name";
pub const STEP_TASK_ID: &str = "task_id";
pub const STEP_KEY: &str = "step";
pub const WORKFLOW_MODEL_KEY: &str = "model";
pub const ACT_OPTIONS_KEY: &str = "options";
pub const ACT_PARAMS_KEY: &str = "params";

pub const ACT_ERR_MESSAGE: &str = "message";
pub const ACT_ERR_CODE: &str = "ecode";

pub const ACT_INDEX: &str = "$index";
pub const ACT_VALUE: &str = "$value";

pub const TASK_EMIT_DISABLED: &str = "__emit_disabled";
pub const TASK_AUOT_COMPLETE: &str = "__auto_complete";

pub const TASK_SIGN: &str = "__sign";
pub const TASK_SIGN_ERR: &str = "error";
pub const TASK_SIGN_CATCH: &str = "catch";
pub const TASK_SIGN_TIMEOUTS: &str = "timeouts";

pub const TASK_TIMEOUTS: &str = "__timeouts";
pub const TASK_COST: &str = "__cost";

pub const ACT_EXPOSE: &str = "exposes";

pub const ACT_TO: &str = "to";

pub const TASK_ROOT_TID: &str = "$";

pub const PROCESS_ID: &str = "pid";
pub const MODEL_ID: &str = "mid";

/// Key delimiter for constructing store keys and composite IDs.
/// Must be valid across all backends (NATS KV, SQL LIKE, Redis).
/// NATS KV allows: [-/_=\.a-zA-Z0-9]
/// SQL LIKE wildcards are _ and %, so these must be escaped or avoided.
pub const KEY_SEP: &str = "-";

#[allow(dead_code)]
pub const ACTS_STORE_NAME: &str = "acts_store";

/// check if the key is private
/// these keys can only be as local data
pub fn is_private_key(key: &str) -> bool {
    key.starts_with("__")
}
