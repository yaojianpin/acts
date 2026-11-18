/// Milliseconds since the Unix epoch — the clock `ActionResult` measures a
/// round trip with.
pub fn time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
