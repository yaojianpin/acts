use super::Role;

/// Person id type.
pub type UserId = String;

/// A person.
#[derive(Debug, Clone)]
pub struct Person {
    /// Person id, supplied by the caller.
    pub id: UserId,
    /// Full name.
    pub name: String,
    /// Role.
    pub role: Role,
}
