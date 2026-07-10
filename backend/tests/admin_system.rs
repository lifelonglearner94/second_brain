//! Integration tests for issue #81: `GET /admin/system` is auth-gated (same
//! `require_session` guard as `/admin/logs`) and returns current host load -
//! CPU, memory, and per-disk usage - plus the mount point holding the Brain
//! File. The assertions pin the response *shape* (types and ranges), not
//! specific values: load varies by host and a CI box may be near-idle, so the
//! contract is "the fields exist and are well-formed," not "CPU is X%."

use axum::body::Body;
use http::header::{COOKIE, SET_COOKIE};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::session::SessionId;
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

#[tokio::test]
async fn admin_system_rejects_missing_cookie() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    // No cookie → 401 (auth-gated, same guard as /admin/logs).
    let (status, body, _) = do_request(&app, "GET", "/admin/system", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "/admin/system without cookie: {body}"
    );

    // A garbage cookie also → 401.
    let bad = http::HeaderValue::from_static("__Host-sb_session=not-a-real-id");
    let (status, _body, _) =
        do_request(&app, "GET", "/admin/system", Some((Value::Null, Some(bad)))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "garbage cookie rejected");
}

#[tokio::test]
async fn admin_system_returns_load_shape_when_authed() {
    // `Db::open(":memory:")` so the bootstrap exception is open (zero users)
    // and `register_and_login` can create the admin. `:memory:` also means
    // `brain_file_mount` is None (no file to resolve to a mount).
    let db = Db::open(":memory:").unwrap();
    let app = routes::router(AppState::for_tests(db));
    let cookie = register_and_login(&app).await;

    let (status, body, _) = do_request(
        &app,
        "GET",
        "/admin/system",
        Some((Value::Null, Some(cookie))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/admin/system authed: {body}");

    // CPU: a usage percent, a core count, and a per-core array whose length
    // matches the core count. Values vary by host (a CI box may be ~0%), so
    // only the shape and ranges are pinned.
    let cpu = &body["cpu"];
    let usage = cpu["usage_percent"]
        .as_f64()
        .expect("cpu.usage_percent is a number");
    assert!(usage >= 0.0, "cpu usage is non-negative: {usage}");
    assert!(usage <= 100.0, "cpu usage is at most 100: {usage}");
    let cores = cpu["cores"].as_u64().expect("cpu.cores is a u64") as usize;
    let per_core = cpu["per_core"]
        .as_array()
        .expect("cpu.per_core is an array");
    assert_eq!(
        per_core.len(),
        cores,
        "per_core length matches cores ({cores})"
    );
    for c in per_core {
        let v = c.as_f64().expect("per_core entry is a number");
        assert!((0.0..=100.0).contains(&v), "per-core usage in [0,100]: {v}");
    }

    // Memory: raw bytes (frontend formats) + a percent. used never exceeds total.
    let mem = &body["memory"];
    let total = mem["total_bytes"]
        .as_u64()
        .expect("memory.total_bytes is a u64");
    let used = mem["used_bytes"]
        .as_u64()
        .expect("memory.used_bytes is a u64");
    let mem_pct = mem["usage_percent"]
        .as_f64()
        .expect("memory.usage_percent is a number");
    assert!(used <= total, "memory used ({used}) <= total ({total})");
    assert!(
        (0.0..=100.0).contains(&mem_pct),
        "memory percent in [0,100]: {mem_pct}"
    );

    // Disks: an array; every entry has name/mount_point strings and byte
    // fields where used <= total. May be empty on a constrained container, so
    // the contract is "if present, well-formed."
    let disks = body["disks"].as_array().expect("disks is an array");
    for d in disks {
        assert!(d["name"].is_string(), "disk.name is a string: {d}");
        assert!(
            d["mount_point"].is_string(),
            "disk.mount_point is a string: {d}"
        );
        let dtotal = d["total_bytes"]
            .as_u64()
            .expect("disk.total_bytes is a u64");
        let dused = d["used_bytes"].as_u64().expect("disk.used_bytes is a u64");
        let dpct = d["usage_percent"]
            .as_f64()
            .expect("disk.usage_percent is a number");
        assert!(dused <= dtotal, "disk used ({dused}) <= total ({dtotal})");
        assert!(
            (0.0..=100.0).contains(&dpct),
            "disk percent in [0,100]: {dpct}"
        );
    }

    // The Brain File (SQLite db) is `:memory:` in tests → no resolvable mount.
    assert!(
        body["brain_file_mount"].is_null(),
        "brain_file_mount is null for :memory: db: {}",
        body["brain_file_mount"]
    );
}
