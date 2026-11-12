use super::UserId;

/// Department id type.
pub type DeptId = String;

/// Department kind, used to look up departments by semantics
/// instead of relying on department name strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeptKind {
    /// Executive office.
    ExecutiveOffice,
    /// R&D department.
    Rnd,
    /// QA department.
    Qa,
    /// Finance department.
    Finance,
}

impl std::fmt::Display for DeptKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExecutiveOffice => "ExecutiveOffice",
            Self::Rnd => "Rnd",
            Self::Qa => "Qa",
            Self::Finance => "Finance",
        })
    }
}

impl std::str::FromStr for DeptKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ExecutiveOffice" => Ok(DeptKind::ExecutiveOffice),
            "Rnd" => Ok(DeptKind::Rnd),
            "Qa" => Ok(DeptKind::Qa),
            "Finance" => Ok(DeptKind::Finance),
            _ => Err(format!("invalid department kind: {s}")),
        }
    }
}

/// A department.
#[derive(Debug, Clone)]
pub struct Department {
    /// Department id, supplied by the caller.
    pub id: DeptId,
    /// Department name. The demo resolves departments by id (`DeptKind` and
    /// `DeptId`), so only the fixture's documentation reads it.
    #[allow(dead_code)] // directory metadata; resources resolve by id
    pub name: String,
    /// Department kind.
    pub kind: DeptKind,
    /// Parent department id; `None` for a top-level department.
    pub parent_id: Option<DeptId>,
    /// Department head (person id).
    pub manager_id: Option<UserId>,
    /// Department members (person ids).
    pub member_ids: Vec<UserId>,
}
