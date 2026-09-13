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

pub const TASK_SIGN: &str = "__sign";
pub const TASK_COST: &str = "__cost";

/// Node ids of the timeout branches that already fired for one task
/// (one-shot guard persisted with the task's vars row — see
/// [`Task::claim_timeout`](crate::scheduler::Task::claim_timeout)).
pub const TASK_TIMEOUTS: &str = "__timeouts";

pub const ACT_TO: &str = "to";

pub const TASK_ROOT_TID: &str = "$";

pub const PROCESS_ID: &str = "pid";
pub const MODEL_ID: &str = "mid";

pub const ACT_RUN_AS_IRQ: &str = "acts.core.irq";
pub const ACT_RUN_AS_MSG: &str = "acts.core.msg";
pub const ACT_RUN_AS: &str = "__run_as";

/// Process env key holding the owner's [`crate::acl::ScopePolicy`] as json.
/// Private (see [`is_private_key`]): the JS `$env` proxy refuses it, so a
/// workflow can neither read nor forge its own scope authority.
pub const PROC_OWNER: &str = "__owner";

/// Start-option key carrying the caller's workdir root from the ACL into
/// `Runtime::start`, which pairs it with the process id to make the
/// process's directory. Popped at start, so it never reaches the workflow.
pub const PROC_WORKDIR_ROOT: &str = "__workdir_root";

/// Process env key holding the directory a process's filesystem access is
/// confined to (`<root>/<pid>`). Private like [`PROC_OWNER`]: the JS `$env`
/// proxy refuses it, and packages read it through `Context::workdir`.
pub const PROC_WORKDIR: &str = "__workdir";

/// Key delimiter for constructing store keys and composite IDs.
/// Must be valid across all backends (NATS KV, SQL LIKE, Redis).
/// NATS KV allows: [-/_=\.a-zA-Z0-9]
/// SQL LIKE wildcards are _ and %, so these must be escaped or avoided.
///
/// `-` is the *only* delimiter byte below the value charset: index-key value
/// segments are encoded (see `encode_key_str`) to never contain `-`, and ids
/// are alphanumeric, so every key char after the value segment is either the
/// `-` separator or a byte > `-`. That keeps each value group a contiguous,
/// monotonic key range addressable by sentinel bounds built from
/// [`KEY_SEP_SUCC`].
pub const KEY_SEP: &str = "-";

/// The byte immediately after [`KEY_SEP`] (`-` = 0x2D -> `.` = 0x2E), used as
/// an exclusive upper bound that covers a whole value group: all keys of
/// value `v` are `..<v>-<id>` < `..<v>.<suffix>`, while the first char of any
/// larger value (`0`..`9`, `A`..`Z`, `a`..`z`, `=`) is strictly greater, so
/// nothing outside the group falls inside the bound.
///
/// `.` never occurs inside an encoded value (`.` maps to `=2E`) or an id.
pub const KEY_SEP_SUCC: &str = ".";

/// check if the key is private
/// these keys can only be as local data
pub fn is_private_key(key: &str) -> bool {
    key.starts_with("__")
}
