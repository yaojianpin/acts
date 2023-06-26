pub const EVT_CREATE: &str = "init";
pub const EVT_ERROR: &str = "error";
pub const EVT_COMPLETE: &str = "complete";
pub const EVT_ABORT: &str = "abort";
pub const EVT_BACK: &str = "back";
pub const EVT_CANCEL: &str = "cancel";
pub const EVT_SUBMIT: &str = "submit";
pub const EVT_SKIP: &str = "skip";
pub const EVT_UPDATE: &str = "update";

pub const RULE_SOME: &str = "some";
pub const RULE_ORD: &str = "ord";
pub const UID: &str = "uid";

pub const SUBJECT_MATCHER: &str = "sub_matcher";
pub const SUBJECT_CANDS: &str = "sub_cands";
pub const SUBJECT_ORD_INDEX: &str = "sub_ord_index";

pub const ACT_OWNER: &str = "owner";
pub const ACT_ACTION: &str = "action";
pub const INITIATOR: &str = "initiator";

pub const AUTO_SUBMIT: &str = "auto_submit";
pub const STEP_ROLE: &str = "role";
pub const STEP_ROLE_SUBMIT: &str = "submit";

pub const ACT_VARS: [&str; 3] = [SUBJECT_MATCHER, SUBJECT_CANDS, STEP_ROLE];
