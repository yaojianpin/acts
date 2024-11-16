pub fn time_millis() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    time.timestamp_millis()
}

pub fn timestamp() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    time.timestamp_micros()
}
