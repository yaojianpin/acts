/// Person role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// President.
    President,
    /// Vice president.
    VicePresident,
    /// President's assistant.
    Assistant,
    /// Department manager.
    DepartmentManager,
    /// Regular employee.
    Employee,
}

impl Role {
    pub fn from_label(s: &str) -> Option<Role> {
        match s {
            "President" => Some(Role::President),
            "Vice President" => Some(Role::VicePresident),
            "Assistant" => Some(Role::Assistant),
            "Department Manager" => Some(Role::DepartmentManager),
            "Employee" => Some(Role::Employee),
            _ => None,
        }
    }
}
