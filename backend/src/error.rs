//! Application error type. Maps cleanly onto HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("sqlite-vec extension load failed: {0}")]
    SqliteVec(String),

    #[error("webauthn error: {0}")]
    WebAuthn(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    /// Issue #74: an invitation was already consumed (or is being reused). 410
    /// Gone - the resource existed but is no longer available for this purpose.
    #[error("gone: {0}")]
    Gone(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("internal error: {0}")]
    Internal(String),

    /// Issue #85: a transient (retryable) failure from the LLM/embedding
    /// provider - Gemini 5xx, overload, rate-limit (429), or a transport
    /// error. The ingest background task retries these on a fixed interval;
    /// they never terminal a braindump. Maps to 503 on the (rare) HTTP path.
    #[error("transient llm error: {0}")]
    TransientLlm(String),

    /// Issue #90: service unavailable (e.g., Deepgram API key not configured)
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
}

impl Error {
    pub fn internal(msg: impl Into<String>) -> Self {
        Error::Internal(msg.into())
    }

    /// Issue #85: whether this error is a transient (retryable) LLM/embedding
    /// provider failure - Gemini 5xx / overloaded / rate-limited / transport.
    /// Only transient errors are retried by the ingest background task;
    /// non-retryable errors (malformed response, logic errors) terminal the
    /// braindump as `failed`.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::TransientLlm(_))
    }
}

impl From<webauthn_rs::prelude::WebauthnError> for Error {
    fn from(e: webauthn_rs::prelude::WebauthnError) -> Self {
        Error::WebAuthn(e.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match &self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Gone(_) => StatusCode::GONE,
            Error::WebAuthn(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::TransientLlm(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = Json(json!({ "error": self.to_string() }));
        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, Error>;
