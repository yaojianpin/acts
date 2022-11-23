pub fn time() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    let time_millis = time.timestamp_millis();

    time_millis
}
