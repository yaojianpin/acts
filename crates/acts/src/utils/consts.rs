pub const INITIATOR: &str = "$initiator";

pub const ACT_USE_PARENT_PROC_ID: &str = "$parent_pid";
pub const ACT_USE_PARENT_TASK_ID: &str = "$parent_tid";

pub const FOR_ACT_KEY_UID: &str = "uid";
pub const STEP_NODE_ID: &str = "node_id";
pub const STEP_NODE_NAME: &str = "node_name";
pub const STEP_TASK_ID: &str = "task_id";
pub const STEP_KEY: &str = "step";
pub const ACT_OPTIONS_KEY: &str = "options";
pub const ACT_PARAMS_KEY: &str = "params";

pub const ACT_ERR_MESSAGE: &str = "message";
pub const ACT_ERR_CODE: &str = "ecode";

pub const ACT_INDEX: &str = "$index";
pub const ACT_VALUE: &str = "$value";

pub const TASK_EMIT_DISABLED: &str = "$emit_disabled";
pub const TASK_AUOT_COMPLETE: &str = "$auto_complete";
pub const IS_CATCH_PROCESSED: &str = "$is_catch_processed";
pub const IS_EVENT_PROCESSED: &str = "$is_event_processed";
pub const IS_TIMEOUT_PROCESSED_PREFIX: &str = "$is_timeout_";

pub const ACT_OUTPUTS: &str = "$outputs";
pub const ACT_PARAMS_CACHE: &str = "$params";

pub const ACT_SUBFLOW_TO: &str = "to";

/// global expose var keys
pub const ACT_GLOBAL_EXPOSE: &str = "expose";

pub const TASK_ROOT_TID: &str = "$";

pub const PROCESS_ID: &str = "pid";
pub const MODEL_ID: &str = "mid";

/// it is used to save the input/output data
/// the data var is default to expose to next task
pub const ACT_DATA: &str = "data";

/// private keys regex
/// these keys can only be as local data
pub const ACT_PRI_KEYS_REGEX: &str = "^(data|__).*";

/// check if the key is private
pub fn is_private_key(key: &str) -> bool {
    key.starts_with("__")
}
