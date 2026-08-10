mod info;
mod message;
mod package;
mod query;

use serde_json::Value;

pub type ActValue = Value;
pub use info::{EventInfo, MessageInfo, ModelInfo, PackageInfo, PageData, ProcInfo, TaskInfo};
pub use message::Message;
pub use package::Package;
pub use query::*;
