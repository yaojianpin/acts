/// Recovery semantics shared by storage, scheduling and durable execution.
///
/// This is deliberately independent of concrete error types. Adapters map
/// their failures into one of these actions; kernel services consume the
/// action without matching strings or storage-specific variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureRecovery {
    /// The decision was built from stale authoritative state. Re-read all
    /// inputs and rebuild the complete guarded operation before retrying.
    RefreshAndRetry,
    /// The operation itself is still valid but bounded capacity or lifecycle
    /// state prevents progress now. Preserve it for a later attempt.
    RetryLater,
    /// Automatic retry would be unsafe or cannot change the outcome.
    Terminal,
}

impl FailureRecovery {
    pub(crate) const fn refreshes_state(self) -> bool {
        matches!(self, Self::RefreshAndRetry)
    }

    pub(crate) const fn is_deferred(self) -> bool {
        matches!(self, Self::RetryLater)
    }

    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_actions_are_mutually_exclusive() {
        for (action, expected) in [
            (FailureRecovery::RefreshAndRetry, (true, false, false)),
            (FailureRecovery::RetryLater, (false, true, false)),
            (FailureRecovery::Terminal, (false, false, true)),
        ] {
            assert_eq!(
                (
                    action.refreshes_state(),
                    action.is_deferred(),
                    action.is_terminal(),
                ),
                expected,
            );
        }
    }
}
