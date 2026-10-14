use super::*;
use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

/// Catches a panic raised while polling one future.
///
/// The engine runs user code (a package's `execute`) and its own hop code on
/// the single driver task: a panic that escaped would kill that task and
/// leave the whole engine wedged with no owner for any operation. A package
/// panic is contained where the package runs and reported as the ordinary
/// package failure (the execution `Failed`, its error walk carries it); a
/// panic anywhere else in a hop is contained around `KernelHost::execute_one`
/// so the driver survives and the operation stays open for recovery.
pub(super) struct CatchPanic<F> {
    future: Option<F>,
}

impl<F> CatchPanic<F> {
    pub(super) fn new(future: F) -> Self {
        Self {
            future: Some(future),
        }
    }
}

impl<F: Future> Future for CatchPanic<F> {
    type Output = std::result::Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // The future is structurally pinned with the wrapper and is never
        // moved or replaced after polling starts.
        let this = unsafe { self.get_unchecked_mut() };
        let Some(future) = this.future.as_mut() else {
            panic!("CatchPanic was polled after completion");
        };
        let future = unsafe { Pin::new_unchecked(future) };

        match catch_unwind(AssertUnwindSafe(move || future.poll(cx))) {
            Ok(Poll::Ready(value)) => {
                this.future = None;
                Poll::Ready(Ok(value))
            }
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => {
                this.future = None;
                Poll::Ready(Err(payload))
            }
        }
    }
}

/// The text of a caught panic, for the error a contained panic is reported as.
pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// The engine error code a lost compare-and-swap carries, so a retryable
/// race is distinguishable from a caller-correctable refusal. It lives
/// with the constructors that read it.
pub(crate) const CONFLICT_ECODE: &str = "ECONFLICT";

pub(super) fn runtime(error: impl std::fmt::Display) -> ActError {
    ActError::Runtime(error.to_string())
}

/// A lost compare-and-swap: retryable, unlike a caller-correctable refusal.
pub(super) fn conflict(message: impl Into<String>) -> ActError {
    ActError::Exception {
        ecode: CONFLICT_ECODE.to_string(),
        message: message.into(),
    }
}

/// Whether an error is a lost compare-and-swap (see [`conflict`]).
pub(super) fn is_conflict(error: &ActError) -> bool {
    matches!(error, ActError::Exception { ecode, .. } if ecode == CONFLICT_ECODE)
}

/// Map a repository failure: a *stale plan* — the aggregate moved between the
/// load a plan was formed from and its commit — is the same retryable race as
/// a lost compare-and-swap, so a caller can re-plan against the fresh
/// snapshot. A plan that would create an operation already standing is that
/// same race, seen from the other side: operation ids are derived from the
/// snapshot (`max(sequence) + 1`, and a hop's own successor), so two writers
/// that loaded the same state mint the same ids, and the second one must
/// re-plan rather than fail (a hard error there leaves a client action refused
/// for a race it never lost). A store-side failure keeps its own typed error (a
/// lost database lease must stay `LeaseLost`: the server stops the engine and
/// the export layer refuses the call on it, neither of which can read it out of
/// a flattened message). Everything else is an engine error.
pub(super) fn repository_error(
    error: crate::runtime::durable::async_document_store::AsyncRepositoryError,
) -> ActError {
    use crate::runtime::core::commit::aggregate::CommitError;
    match &error {
        crate::runtime::durable::async_document_store::AsyncRepositoryError::Store(error) => {
            error.clone()
        }
        crate::runtime::durable::async_document_store::AsyncRepositoryError::Domain(
            CommitError::VersionConflict { .. } | CommitError::OperationAlreadyExists(_),
        ) => conflict(format!("stale plan: {error}")),
        _ => runtime(error),
    }
}
