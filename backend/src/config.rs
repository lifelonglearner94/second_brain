//! Environment-driven configuration. No secrets live in the repo.

use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    /// Socket address to bind, e.g. "0.0.0.0:8080".
    pub bind_addr: String,
    /// SQLite path (file) or `:memory:` for tests.
    pub database_url: String,
    /// `json` (default) or `plain` — controls the tracing subscriber format.
    pub log_format: LogFormat,
    /// Value for the `RUST_LOG`-style env filter.
    pub rust_log: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Plain,
}

impl Config {
    /// Read configuration from the process environment, with sensible defaults.
    pub fn from_env() -> Self {
        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        let port = env::var("PORT").unwrap_or_else(|_| "8080".to_string());
        let log_format = match env::var("LOG_FORMAT")
            .unwrap_or_else(|_| "json".to_string())
            .as_str()
        {
            "plain" => LogFormat::Plain,
            _ => LogFormat::Json,
        };
        Self {
            bind_addr: format!("{host}:{port}"),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "second_brain.db".to_string()),
            rust_log: env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,second_brain_backend=debug".to_string()),
            log_format,
        }
    }

    /// A configuration suitable for in-process tests.
    pub fn for_tests() -> Self {
        Self {
            bind_addr: "0.0.0.0:0".to_string(),
            database_url: ":memory:".to_string(),
            log_format: LogFormat::Plain,
            rust_log: "warn".to_string(),
        }
    }
}
