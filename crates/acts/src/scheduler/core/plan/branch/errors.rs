use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BranchError {
    InvalidDecision(String),
    InvalidTargetTaskId(String),
}
impl fmt::Display for BranchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "branch error: {self:?}")
    }
}
impl std::error::Error for BranchError {}
