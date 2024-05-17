pub fn time_millis() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    let time_millis = time.timestamp_millis();

    time_millis
}

pub fn timestamp() -> i64 {
    let time: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    time.timestamp_micros()
}
