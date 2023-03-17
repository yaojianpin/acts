#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
      #[cfg(feature = "debug")]
      tracing::debug!($($arg)*);
    };
}

#[macro_export]
macro_rules! db_debug {
  ($($arg:tt)*) => {
    #[cfg(feature = "db_debug")]
    tracing::debug!($($arg)*);
  };
}
