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
    /// WebAuthn relying-party id (effective domain of `webauthn_rp_origin`).
    pub webauthn_rp_id: String,
    /// WebAuthn relying-party origin, e.g. `https://brain.example.com`.
    pub webauthn_rp_origin: String,
    /// Human-friendly relying-party name shown to the user by authenticators.
    pub webauthn_rp_name: String,
    /// Issue #85: seconds between ingest retry attempts after a transient
    /// (Gemini 5xx / rate-limited / transport) failure. ~3 minutes keeps the
    /// retry off the hot path while an overloaded provider recovers; the
    /// startup recovery scan ignores the interval and attempts immediately.
    pub ingest_retry_interval_secs: u64,
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
            webauthn_rp_id: env::var("WEBAUTHN_RP_ID").unwrap_or_else(|_| "localhost".to_string()),
            webauthn_rp_origin: env::var("WEBAUTHN_RP_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            webauthn_rp_name: env::var("WEBAUTHN_RP_NAME")
                .unwrap_or_else(|_| "Second Brain".to_string()),
            log_format,
            ingest_retry_interval_secs: env::var("INGEST_RETRY_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(180),
        }
    }

    /// A configuration suitable for in-process tests.
    pub fn for_tests() -> Self {
        Self {
            bind_addr: "0.0.0.0:0".to_string(),
            database_url: ":memory:".to_string(),
            log_format: LogFormat::Plain,
            rust_log: "warn".to_string(),
            webauthn_rp_id: "localhost".to_string(),
            webauthn_rp_origin: "http://localhost:8080".to_string(),
            webauthn_rp_name: "Second Brain (test)".to_string(),
            // Zero so the inline-test runner (and any retry test using the
            // default) never blocks on a sleep — the FakeLlm seam succeeds on
            // the first attempt, so the loop exits immediately.
            ingest_retry_interval_secs: 0,
        }
    }
}
