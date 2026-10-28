//! The process-lifecycle port: reporting a run's start and terminal event.

/// Which engine event a run's lifecycle report is: the driver's mirror of the
/// legacy `on_proc` emission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessEvent {
    Started,
    Completed,
    Errored,
}

/// The process-lifecycle sink: the engine routes it onto its channel events
/// (`on_start`/`on_complete`/`on_error`).
///
/// This is the unsized callable the kernel stores in a `PortSet`; a
/// registration supplies an `Arc` of a closure. The host calls it
/// synchronously after the commit and its projection.
pub(crate) type LifecycleSink = dyn Fn(&crate::Message, ProcessEvent) + Send + Sync;
