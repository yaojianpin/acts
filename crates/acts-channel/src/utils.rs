/// Milliseconds since the Unix epoch — the clock `ActionResult` measures a
/// round trip with.
pub fn time_millis() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    time.timestamp_millis()
}
