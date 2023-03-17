mod convert;
mod id;
pub(crate) mod log;
mod macros;
pub(crate) mod time;
pub(crate) mod vars;

pub use convert::*;
pub use id::*;

use crate::options::Options;

pub fn default_config() -> Options {
    Options {
        cache_cap: 100,
        scher_cap: 20,
    }
}
