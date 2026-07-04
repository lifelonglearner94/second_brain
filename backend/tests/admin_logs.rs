//! Integration tests for issue #4: `GET /admin/logs` is auth-gated and returns
//! the backend's recent structured logs from the in-memory ring buffer.
//!
//! Auth rejection (no cookie → 401) and a successful authenticated fetch,
//! including the `?limit` bound that keeps the response VPS-safe.

use axum::body::Body;
use http::header::{COOKIE, SET_COOKIE};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::session::SessionId;
use second_brain_backend::logs::LogEntry;
use second_brain_backend::{db::Db, routes, state::AppState};
use serde_json::Value;
use tower::ServiceExt;
use webauthn_authenticator_rs::prelude::{Url, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs_proto::{CreationChallengeResponse, RequestChallengeResponse};

const ORIGIN: &str = "http://localhost:8080";

/// Run a request against the app and collect (status, body-as-Value, headers).
async fn do_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<(Value, Option<http::HeaderValue>)>,
) -> (StatusCode, Value, http::HeaderMap) {
    let cookie = body.as_ref().and_then(|(_, c)| c.clone());
    let request = match body {
        Some((b, _)) => {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json");
            if let Some(c) = cookie {
                builder = builder.header(COOKIE, c);
            }
            builder.body(Body::from(b.to_string())).unwrap()
        }
        None => {
            let mut builder = Request::builder().method(method).uri(uri);
            if let Some(c) = cookie {
                builder = builder.header(COOKIE, c);
            }
            builder.body(Body::empty()).unwrap()
        }
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
    (status, value, headers)
}

/// Pull the session id out of the login `Set-Cookie` and wrap it as a
/// `Cookie: __Host-sb_session=<id>` request header value.
fn login_cookie_header(headers: &http::HeaderMap) -> http::HeaderValue {
    let raw = headers
        .get(SET_COOKIE)
        .expect("login_finish must set a Set-Cookie")
        .to_str()
        .unwrap();
    let after_eq = raw.split("__Host-sb_session=").nth(1).unwrap();
    let id = after_eq.split(';').next().unwrap();
    request_cookie_header_value(&SessionId::parse(id).expect("well-formed session id"))
}

/// Register a soft passkey and log in, returning the `Cookie` request header
/// for the resulting session.
async fn register_and_login(app: &axum::Router) -> http::HeaderValue {
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let origin = Url::parse(ORIGIN).unwrap();

    let (_, body, _) = do_request(app, "POST", "/auth/register/begin", None).await;
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: CreationChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    let credential = authenticator
        .do_registration(origin.clone(), challenge)
        .expect("soft passkey registers");
    let finish = serde_json::json!({ "credential": credential, "state": state });
    let (_, _, _) = do_request(app, "POST", "/auth/register/finish", Some((finish, None))).await;

    let (_, body, _) = do_request(app, "POST", "/auth/login/begin", None).await;
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: RequestChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    let credential = authenticator
        .do_authentication(origin, challenge)
        .expect("soft passkey authenticates");
    let finish = serde_json::json!({ "credential": credential, "state": state });
    let (_, _, headers) = do_request(app, "POST", "/auth/login/finish", Some((finish, None))).await;
    login_cookie_header(&headers)
}

fn entry(message: &str, level: &str) -> LogEntry {
    LogEntry {
        timestamp: 1_700_000_000,
        level: level.to_string(),
        target: "gemini_client".to_string(),
        message: message.to_string(),
        fields: serde_json::json!({ "status": 503 }),
    }
}

#[tokio::test]
async fn admin_logs_rejects_missing_cookie() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    // No cookie → 401 (auth-gated, same guard as /me).
    let (status, body, _) = do_request(&app, "GET", "/admin/logs", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "/admin/logs without cookie: {body}"
    );

    // A garbage cookie also → 401.
    let bad = http::HeaderValue::from_static("__Host-sb_session=not-a-real-id");
    let (status, _body, _) =
        do_request(&app, "GET", "/admin/logs", Some((Value::Null, Some(bad)))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "garbage cookie rejected");
}

#[tokio::test]
async fn admin_logs_returns_recent_entries_when_authed() {
    let db = Db::open_in_memory().unwrap();
    let state = AppState::for_tests(db);
    // Pre-populate the shared buffer (Arc-backed, so the router's clone sees
    // the same entries) with a couple of structured log lines — the kind of
    // thing the admin tab exists to surface.
    state.log_buffer.push(entry("generation failed", "ERROR"));
    state.log_buffer.push(entry("retrying", "WARN"));
    let app = routes::router(state);

    let cookie = register_and_login(&app).await;

    let (status, body, _) = do_request(
        &app,
        "GET",
        "/admin/logs",
        Some((Value::Null, Some(cookie))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/admin/logs authed: {body}");
    assert_eq!(body["count"], 2, "both retained entries returned: {body}");
    assert_eq!(
        body["capacity"], 1_000,
        "capacity surfaced for the admin tab"
    );
    let logs = body["logs"].as_array().expect("logs is an array");
    assert_eq!(logs[0]["message"], "generation failed");
    assert_eq!(logs[0]["level"], "ERROR");
    assert_eq!(logs[0]["fields"]["status"], 503);
    assert_eq!(logs[1]["message"], "retrying");
    assert_eq!(logs[1]["level"], "WARN");
}

#[tokio::test]
async fn admin_logs_limit_bounds_the_response() {
    let db = Db::open_in_memory().unwrap();
    let state = AppState::for_tests(db);
    for i in 0..5 {
        state.log_buffer.push(entry(&format!("e{i}"), "INFO"));
    }
    let app = routes::router(state);
    let cookie = register_and_login(&app).await;

    let (status, body, _) = do_request(
        &app,
        "GET",
        "/admin/logs?limit=2",
        Some((Value::Null, Some(cookie))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "?limit fetch: {body}");
    assert_eq!(body["count"], 2, "limit caps the returned count");
    let logs = body["logs"].as_array().unwrap();
    // Newest two only, oldest-first (chronological).
    assert_eq!(logs[0]["message"], "e3");
    assert_eq!(logs[1]["message"], "e4");
}
