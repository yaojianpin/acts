//! Capability types and provider descriptions.
//!
//! These are descriptions only: this module neither registers a provider nor
//! selects one for a model. Registration and publish/start checks are a later
//! task.

/// A capability type the kernel defines.
///
/// The kind names the kernel trait a port set holds; it is not a plugin name.
/// A plugin advertises a provider key of its own for a kind, and the kernel
/// maps a model attribute to the kind without depending on plugin registration
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityKind {
    /// Resolving the participants an owner expression names (`assign.owner`).
    Participant,
}

impl CapabilityKind {
    /// The diagnostic name of the kind.
    ///
    /// This is for logs and error messages, not a provider key: a plugin that
    /// provides the capability chooses its own key.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Participant => "participant",
        }
    }

    /// The model attribute this capability backs.
    ///
    /// The attribute is the stable model vocabulary (`assign.owner`); the
    /// provider key a plugin chooses is separate.
    pub fn attribute(&self) -> &'static str {
        match self {
            Self::Participant => "assign.owner",
        }
    }
}

/// A capability implementation a plugin advertises.
///
/// `key` is the plugin's external provider key; `plugin` and `version`
/// identify who provides it, so a publish or start failure can name the
/// mismatch. The registry that stores these descriptions is a later task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityProvider {
    pub kind: CapabilityKind,
    pub key: String,
    pub plugin: String,
    pub version: String,
}

/// A capability a model requires.
///
/// `key` is the provider the model explicitly chose, if any. A `None` key
/// leaves provider selection to the later registration/selection task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub kind: CapabilityKind,
    pub key: Option<String>,
}
