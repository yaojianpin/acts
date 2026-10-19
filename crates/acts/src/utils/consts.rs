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

pub const TASK_COST: &str = "__cost";

/// Node ids of the timeout branches that already fired for one task (the
/// one-shot guard a timeout branch's evaluation keeps in the run's own
/// state).
pub const TASK_TIMEOUTS: &str = "__timeouts";

/// The per-item bindings a param fan-out (`acts.core.parallel{in}` /
/// `acts.core.sequence{in}`) pins on each item's template execution: the
/// item's position and value.
///
/// They are private (`__`-prefixed) on purpose — engine facts, not run data:
/// they never fold into an ancestor scope (`PropagateTask` skips private
/// names), never reach a task's projected row, a run's outputs or a message's
/// `inputs`, and a workflow variable of the same public name is therefore
/// never shadowed by them. Expressions still read them as `index`/`value` (the
/// names the container packages' inline acts declare), through the alias in
/// `scheduler::shell::facts::scope_vars`.
pub(crate) const FANOUT_INDEX_VAR: &str = "__fanout_index";
pub(crate) const FANOUT_VALUE_VAR: &str = "__fanout_value";

pub const ACT_TO: &str = "to";

pub const TASK_ROOT_TID: &str = "$";

pub const PROCESS_ID: &str = "pid";
pub const MODEL_ID: &str = "mid";

pub const ACT_RUN_AS_IRQ: &str = "acts.core.irq";
pub const ACT_RUN_AS_MSG: &str = "acts.core.msg";

/// Process env key holding the owner's [`crate::acl::ScopePolicy`] as json.
/// Private (see [`is_private_key`]): the JS `$env` proxy refuses it, so a
/// workflow can neither read nor forge its own scope authority.
pub const PROC_OWNER: &str = "__owner";

/// Process env key holding the stable idempotency key of the effect currently
/// executing. Private (see [`is_private_key`]): the workflow's `$env` proxy
/// refuses it and the effect's write-back filters it, so it never becomes
/// durable run data. Packages read it through `Context::effect_key`.
pub const EFFECT_KEY: &str = "__effect_key";

/// Process env key holding the process's own directory
/// (`<workdir>/<pid>`). Private like [`PROC_OWNER`]: the JS `$env`
/// proxy refuses it, and packages read it through `Context::workdir`.
pub const PROC_WORKDIR: &str = "__workdir";

/// The name a workflow reads its process directory under: `$env.WORK_DIR` is
/// the same `<root>/<pid>` that `Context::workdir()` answers, not the root the
/// engine config declares. Engine-owned and read-only — `Context::get_env`
/// answers it from the process itself (never a stored value or an OS variable
/// of the same name) and `Context::set_env` drops a write to it, so a script
/// can find the directory it runs in but cannot redefine it.
pub const ENV_WORK_DIR: &str = "WORK_DIR";

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
