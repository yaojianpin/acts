#[macro_export]
macro_rules! include_json {
    ($file:expr) => {{
        let json_str = include_str!($file);
        serde_json::from_str::<serde_json::Value>(json_str)
            .expect(&format!("Failed to parse JSON file: {}", $file))
    }};
}
