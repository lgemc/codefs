//! Centralized logging module for CodeFS.
//!
//! This module provides a unified logging interface that reads configuration from
//! environment variables. Use the `CODEFS_LOG` environment variable to control
//! log levels:
//!
//! - `error` - Only errors
//! - `warn`  - Warnings and errors
//! - `info`  - Info, warnings, and errors
//! - `debug` - Debug messages and above (detailed operational info)
//! - `trace` - All messages including fine-grained tracing
//!
//! You can also use `RUST_LOG` for more granular control per module.
//!
//! # Examples
//!
//! ```bash
//! # Set global log level
//! CODEFS_LOG=debug codefs mount --source ./src --mountpoint ./mnt
//!
//! # Or use RUST_LOG for per-module control
//! RUST_LOG=codefs=debug,vfs_core=trace codefs mount ...
//! ```

use tracing::Level;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
};

/// Log level configuration parsed from environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Only errors
    Error,
    /// Warnings and errors
    Warn,
    /// Info, warnings, and errors
    Info,
    /// Debug messages and above
    Debug,
    /// All messages including trace
    Trace,
}

impl LogLevel {
    /// Parse log level from string (case-insensitive).
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    /// Convert to tracing Level.
    pub fn to_tracing_level(self) -> Level {
        match self {
            Self::Error => Level::ERROR,
            Self::Warn => Level::WARN,
            Self::Info => Level::INFO,
            Self::Debug => Level::DEBUG,
            Self::Trace => Level::TRACE,
        }
    }

    /// Convert to filter directive string.
    pub fn as_filter_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Warn
    }
}

/// Configuration for the centralized logger.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// The log level to use.
    pub level: LogLevel,
    /// Whether to include timestamps.
    pub with_timestamps: bool,
    /// Whether to include the target (module path).
    pub with_target: bool,
    /// Whether to include file and line numbers.
    pub with_file_line: bool,
    /// Whether to include span events (enter/exit).
    pub with_span_events: bool,
    /// Whether to use ANSI colors.
    pub with_ansi: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            with_timestamps: true,
            with_target: true,
            with_file_line: false,
            with_span_events: false,
            with_ansi: true,
        }
    }
}

impl LogConfig {
    /// Create a new log configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the log level.
    pub fn level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }

    /// Enable or disable timestamps.
    pub fn with_timestamps(mut self, enabled: bool) -> Self {
        self.with_timestamps = enabled;
        self
    }

    /// Enable or disable target (module path).
    pub fn with_target(mut self, enabled: bool) -> Self {
        self.with_target = enabled;
        self
    }

    /// Enable or disable file and line numbers.
    pub fn with_file_line(mut self, enabled: bool) -> Self {
        self.with_file_line = enabled;
        self
    }

    /// Enable or disable span events.
    pub fn with_span_events(mut self, enabled: bool) -> Self {
        self.with_span_events = enabled;
        self
    }

    /// Enable or disable ANSI colors.
    pub fn with_ansi(mut self, enabled: bool) -> Self {
        self.with_ansi = enabled;
        self
    }

    /// Create configuration from environment variables.
    ///
    /// Checks `CODEFS_LOG` first, then falls back to parsing `RUST_LOG`,
    /// and finally uses the CLI verbosity level.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Check CODEFS_LOG first
        if let Ok(level_str) = std::env::var("CODEFS_LOG") {
            if let Some(level) = LogLevel::from_str(&level_str) {
                config.level = level;
            }
        }
        // Fall back to RUST_LOG if set (will be handled by EnvFilter)
        else if std::env::var("RUST_LOG").is_ok() {
            // RUST_LOG is set, we'll use EnvFilter to parse it
            // Default to trace here so EnvFilter can do the filtering
            config.level = LogLevel::Trace;
        }

        // Check for additional configuration
        if std::env::var("CODEFS_LOG_TIMESTAMPS").map(|v| v == "0" || v == "false").unwrap_or(false) {
            config.with_timestamps = false;
        }

        if std::env::var("CODEFS_LOG_FILE_LINE").map(|v| v == "1" || v == "true").unwrap_or(false) {
            config.with_file_line = true;
        }

        if std::env::var("NO_COLOR").is_ok() {
            config.with_ansi = false;
        }

        config
    }

    /// Create configuration from CLI verbosity level.
    pub fn from_verbosity(verbosity: u8) -> Self {
        let level = match verbosity {
            0 => LogLevel::Warn,
            1 => LogLevel::Info,
            2 => LogLevel::Debug,
            _ => LogLevel::Trace,
        };
        Self::default().level(level)
    }
}

/// Initialize the global logger with the given configuration.
///
/// This should be called once at application startup.
///
/// # Errors
///
/// Returns an error if the logger has already been initialized.
pub fn init(config: &LogConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let env_filter = if std::env::var("RUST_LOG").is_ok() {
        // Use RUST_LOG for fine-grained control
        EnvFilter::from_default_env()
    } else {
        // Use our level configuration
        EnvFilter::new(config.level.as_filter_str())
    };

    let span_events = if config.with_span_events {
        FmtSpan::ENTER | FmtSpan::EXIT
    } else {
        FmtSpan::NONE
    };

    let subscriber = tracing_subscriber::registry().with(env_filter).with(
        fmt::layer()
            .with_ansi(config.with_ansi)
            .with_target(config.with_target)
            .with_file(config.with_file_line)
            .with_line_number(config.with_file_line)
            .with_span_events(span_events)
            .with_timer(if config.with_timestamps {
                fmt::time::uptime()
            } else {
                fmt::time::uptime() // We'll handle this differently
            }),
    );

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(())
}

/// Initialize logging from environment variables.
///
/// Convenience function that calls `init(LogConfig::from_env())`.
pub fn init_from_env() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init(&LogConfig::from_env())
}

/// Initialize logging from CLI verbosity.
///
/// Convenience function for CLI applications.
pub fn init_from_verbosity(verbosity: u8) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check if env vars override CLI
    if std::env::var("CODEFS_LOG").is_ok() || std::env::var("RUST_LOG").is_ok() {
        init_from_env()
    } else {
        init(&LogConfig::from_verbosity(verbosity))
    }
}

/// Macro for logging with consistent formatting across the codebase.
/// Re-exports tracing macros for convenience.
pub use tracing::{debug, error, info, trace, warn};

/// Span for tracing operations.
pub use tracing::span;

/// Instrument attribute for async functions.
pub use tracing::instrument;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_from_str() {
        assert_eq!(LogLevel::from_str("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::from_str("invalid"), None);
    }

    #[test]
    fn test_config_builder() {
        let config = LogConfig::new()
            .level(LogLevel::Debug)
            .with_timestamps(false)
            .with_target(true)
            .with_file_line(true);

        assert_eq!(config.level, LogLevel::Debug);
        assert!(!config.with_timestamps);
        assert!(config.with_target);
        assert!(config.with_file_line);
    }

    #[test]
    fn test_verbosity_mapping() {
        assert_eq!(LogConfig::from_verbosity(0).level, LogLevel::Warn);
        assert_eq!(LogConfig::from_verbosity(1).level, LogLevel::Info);
        assert_eq!(LogConfig::from_verbosity(2).level, LogLevel::Debug);
        assert_eq!(LogConfig::from_verbosity(3).level, LogLevel::Trace);
        assert_eq!(LogConfig::from_verbosity(10).level, LogLevel::Trace);
    }
}
