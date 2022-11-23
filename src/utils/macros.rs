#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
      #[cfg(feature = "debug")]
      println!($($arg)*);
    };
}

#[macro_export]
macro_rules! db_debug {
  ($($arg:tt)*) => {
    #[cfg(feature = "db_debug")]
    println!($($arg)*);
  };
}
