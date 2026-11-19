//! The live ack-channel registry the durable message publisher consults.
//!
//! A delivery row is one channel's responsibility for a message. Creating it
//! after the outbox record was confirmed left a crash window in which the
//! canonical message existed but no delivery did, so no retry could ever find
//! it. The publisher therefore asks this registry, before the outbox
//! confirmation, which registered ack channels match the message and writes
//! their deterministic delivery rows first.
//!
//! Registration is intentionally in-memory: it describes the currently
//! connected subscribers. Rows created for a subscriber survive a restart and
//! are re-sent by the retry timer when that subscriber registers again.

use crate::Message;
use std::collections::HashMap;
use std::sync::Arc;

/// One registered channel's message filter.
#[derive(Clone)]
pub(crate) struct DeliveryTarget {
    pub(crate) chan_id: String,
    pub(crate) pattern: String,
    pub(crate) ack: bool,
    pub(crate) matches: Arc<dyn Fn(&Message) -> bool + Send + Sync>,
}

/// The channels currently registered on the engine, keyed by channel id (the
/// same key the emitter uses, so a re-subscription replaces its target).
#[derive(Default)]
pub(crate) struct DeliveryTargets {
    targets: parking_lot::RwLock<HashMap<String, DeliveryTarget>>,
}

impl DeliveryTargets {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&self, target: DeliveryTarget) {
        self.targets.write().insert(target.chan_id.clone(), target);
    }

    pub(crate) fn unregister(&self, chan_id: &str) {
        self.targets.write().remove(chan_id);
    }

    /// Every ack-requiring channel whose filter matches `message`, in channel
    /// id order. Non-ack channels still receive the online notification; they
    /// have no delivery row to create.
    pub(crate) fn matching(&self, message: &Message) -> Vec<DeliveryTarget> {
        let mut targets: Vec<_> = self
            .targets
            .read()
            .values()
            .filter(|target| target.ack && (target.matches)(message))
            .cloned()
            .collect();
        targets.sort_by(|left, right| left.chan_id.cmp(&right.chan_id));
        targets
    }

    pub(crate) fn clear(&self) {
        self.targets.write().clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.targets.read().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageState, Vars};

    fn message(id: &str) -> Message {
        Message {
            id: id.to_string(),
            state: MessageState::Created,
            r#type: "step".into(),
            pid: "p1".into(),
            tid: "t1".into(),
            inputs: Vars::new(),
            outputs: Vars::new(),
            ..Message::default()
        }
    }

    fn target(id: &str, ack: bool, matching: bool) -> DeliveryTarget {
        DeliveryTarget {
            chan_id: id.to_string(),
            pattern: "{}".into(),
            ack,
            matches: Arc::new(move |_| matching),
        }
    }

    #[test]
    fn matching_filters_ack_and_sorts_by_channel() {
        let targets = DeliveryTargets::new();
        targets.register(target("b", true, true));
        targets.register(target("a", true, true));
        targets.register(target("c", false, true));
        targets.register(target("d", true, false));

        let matched = targets.matching(&message("m1"));
        assert_eq!(
            matched
                .iter()
                .map(|target| target.chan_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn registration_replaces_and_unregister_removes() {
        let targets = DeliveryTargets::new();
        targets.register(target("a", true, false));
        assert!(targets.matching(&message("m1")).is_empty());
        targets.register(target("a", true, true));
        assert_eq!(targets.matching(&message("m1")).len(), 1);
        assert_eq!(targets.len(), 1);
        targets.unregister("a");
        assert!(targets.matching(&message("m1")).is_empty());
    }
}
