//! Logging utilities for imekit
//!
//! This module provides conditional logging macros that work with either
//! the `log` crate or `tracing` crate depending on which feature is enabled.
//!
//! ## Feature Selection
//!
//! - Enable the `log` feature to use the `log` crate for logging
//! - Enable the `tracing` feature to use the `tracing` crate for logging
//! - If neither feature is enabled, logging macros are no-ops (silent)
//!
//! ## Example
//!
//! ```toml
//! [dependencies]
//! imekit = { version = "0.1", features = ["log"] }
//! # or
//! imekit = { version = "0.1", features = ["tracing"] }
//! ```

/// Log a warning message
///
/// When neither `log` nor `tracing` feature is enabled, this is a no-op.
#[macro_export]
#[doc(hidden)]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        #[cfg(feature = "log")]
        ::log::warn!($($arg)*);
        #[cfg(feature = "tracing")]
        ::tracing::warn!($($arg)*);
        #[cfg(not(any(feature = "log", feature = "tracing")))]
        {
            _ = format!($($arg)*);
        }
    };
}

/// Log a debug message
///
/// When neither `log` nor `tracing` feature is enabled, this is a no-op.
#[macro_export]
#[doc(hidden)]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        #[cfg(feature = "log")]
        ::log::debug!($($arg)*);
        #[cfg(feature = "tracing")]
        ::tracing::debug!($($arg)*);
        #[cfg(not(any(feature = "log", feature = "tracing")))]
        {
            _ = format!($($arg)*);
        }
    };
}

/// Log an info message
///
/// When neither `log` nor `tracing` feature is enabled, this is a no-op.
#[macro_export]
#[doc(hidden)]
macro_rules! log_info {
    ($($arg:tt)*) => {
        #[cfg(feature = "log")]
        ::log::info!($($arg)*);
        #[cfg(feature = "tracing")]
        ::tracing::info!($($arg)*);
        #[cfg(not(any(feature = "log", feature = "tracing")))]
        {
            _ = format!($($arg)*);
        }
    };
}

/// Log an error message
///
/// When neither `log` nor `tracing` feature is enabled, this is a no-op.
#[macro_export]
#[doc(hidden)]
macro_rules! log_error {
    ($($arg:tt)*) => {
        #[cfg(feature = "log")]
        ::log::error!($($arg)*);
        #[cfg(feature = "tracing")]
        ::tracing::error!($($arg)*);
        #[cfg(not(any(feature = "log", feature = "tracing")))]
        {
            _ = format!($($arg)*);
        }
    };
}

/// Log a trace message
///
/// When neither `log` nor `tracing` feature is enabled, this is a no-op.
#[macro_export]
#[doc(hidden)]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        #[cfg(feature = "log")]
        ::log::trace!($($arg)*);
        #[cfg(feature = "tracing")]
        ::tracing::trace!($($arg)*);
        #[cfg(not(any(feature = "log", feature = "tracing")))]
        {
            _ = format!($($arg)*);
        }
    };
}
