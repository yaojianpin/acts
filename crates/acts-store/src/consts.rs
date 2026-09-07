/// Name of the SQL table / NATS KV bucket the backends store keys in.
///
/// MUST stay byte-identical with `acts::utils::consts::ACTS_STORE_NAME` (the
/// acts crate keeps that const crate-private): SQL and JetStream resources
/// are identified by this name alone, so a drift would silently orphan data
/// written by the other side.
#[allow(dead_code)]
pub(crate) const ACTS_STORE_NAME: &str = "acts_store";
