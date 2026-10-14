/// Why a wake-up could not be delivered.
///
/// A wake is only an optimization over already durable operations: losing it
/// is safe because recovery scans the same repository and re-drives whatever
/// no owner moved. The message is carried for the operator, not acted on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeError(pub(crate) String);
