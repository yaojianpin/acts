/// Why a wake-up could not be delivered.
///
/// A wake is only an optimization over already durable operations: losing it
/// is safe because the ready index holds the same record and the driver's due
/// pass reads it. The message is carried for the operator, not acted on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WakeError(pub(crate) String);
