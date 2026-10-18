//! The stable identity of one external effect.
//!
//! An effect is addressed by its operation: the operation's identity is
//! durable before the effect runs (`ClaimAndPrepare` writes it), so the key is
//! a *pure function* of persisted facts — process, node, generation and the
//! operation sequence. There is deliberately no random component and no
//! separate stored copy: a key minted from the same operation after a lost
//! compare-and-swap, a recovery claim or a process restart is byte-identical.
//!
//! The rendered form is the value a transport/executor may use as an external
//! idempotency token. It is the C5 instance wire (`sha256(pid, node,
//! generation)[..32]`, an alphanumeric id) plus the sequence, so it is stable,
//! injective for one operation and safe in store keys and request headers.

use super::{operation::OperationId, task_graph::TaskId};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct EffectKey {
    pub(crate) process_id: String,
    pub(crate) task: TaskId,
    pub(crate) sequence: u64,
}

impl EffectKey {
    /// The effect key of `id`. Every operation owns at most one effect, so the
    /// operation identity *is* the effect identity; a re-planned or recovered
    /// operation keeps the key it was claimed under.
    pub(crate) fn of(id: &OperationId) -> Self {
        Self {
            process_id: id.process_id.clone(),
            task: id.task.clone(),
            sequence: id.sequence,
        }
    }

    /// The stable external form: `<instance wire>-<sequence>`.
    pub(crate) fn wire(&self) -> String {
        format!("{}-{}", self.task.wire(&self.process_id), self.sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(sequence: u64) -> OperationId {
        OperationId {
            process_id: "p1".into(),
            task: TaskId::new("step", 2).unwrap(),
            sequence,
        }
    }

    #[test]
    fn the_key_is_a_pure_function_of_the_operation_identity() {
        let first = EffectKey::of(&id(7));
        let rebuilt = EffectKey::of(&id(7));
        assert_eq!(first, rebuilt);
        assert_eq!(first.wire(), rebuilt.wire());
        assert!(
            first.wire().starts_with(&id(7).task.wire("p1")),
            "the key carries the durable instance wire: {}",
            first.wire()
        );
    }

    #[test]
    fn different_operations_have_different_keys() {
        assert_ne!(EffectKey::of(&id(7)).wire(), EffectKey::of(&id(8)).wire());
        assert_ne!(
            EffectKey::of(&id(7)).wire(),
            EffectKey::of(&OperationId {
                process_id: "p2".into(),
                task: id(7).task,
                sequence: 7,
            })
            .wire()
        );
    }

    #[test]
    fn the_key_contains_no_random_or_volatile_component() {
        // Two calls in different "times"/restarts (same durable identity) can
        // only be equal if nothing volatile participates in the key.
        let derived = EffectKey::of(&id(3)).wire();
        assert_eq!(derived, EffectKey::of(&id(3)).wire());
        assert_eq!(derived.matches('-').count(), 1);
    }
}
