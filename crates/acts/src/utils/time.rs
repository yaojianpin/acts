pub fn time_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub fn timestamp() -> i64 {
    jiff::Timestamp::now().as_microsecond()
}
