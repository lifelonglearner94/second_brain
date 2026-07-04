//! Integration tests for issue #2: passkey register → login → protected →
//! logout, using a software passkey (`SoftPasskey`) so no hardware is needed,
//! plus the rejected no-cookie request and a stale-cookie-after-logout case.

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
use webauthn_rs_proto::PublicKeyCredential;
use webauthn_rs_proto::{CreationChallengeResponse, RequestChallengeResponse};

const ORIGIN: &str = "http://localhost:8080";

/// Helper: run a request against the app and collect (status, body-as-Value, headers).
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

/// Pull the `Set-Cookie: __Host-sb_session=<id>; ...` value's `<id>` out.
fn session_id_from_set_cookie(headers: &http::HeaderMap) -> String {
    let raw = headers
        .get(SET_COOKIE)
        .expect("login_finish must set a Set-Cookie")
        .to_str()
        .unwrap();
    let after_eq = raw.split("__Host-sb_session=").nth(1).unwrap();
    let id = after_eq.split(';').next().unwrap();
    id.to_string()
}

#[tokio::test]
async fn full_register_login_protected_logout_flow() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    // `falsify_uv = true`: the SoftPasskey claims user-verification it can't really
    // perform, which is what the server's `UserVerificationPolicy::Required` demands.
    // It's a test-only assertion; real authenticators do this honestly.
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let origin = Url::parse(ORIGIN).unwrap();

    // --- register begin ---
    let (status, body, _h) = do_request(&app, "POST", "/auth/register/begin", None).await;
    assert_eq!(status, StatusCode::OK, "register begin: {body}");
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: CreationChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();

    // --- soft passkey produces a credential ---
    let credential = authenticator
        .do_registration(origin.clone(), challenge)
        .expect("soft passkey registers");

    // --- register finish ---
    let finish_body = serde_json::json!({
        "credential": credential,
        "state": state,
    });
    let (status, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/finish",
        Some((finish_body, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register finish: {body}");

    // --- login begin ---
    let (status, body, _h) = do_request(&app, "POST", "/auth/login/begin", None).await;
    assert_eq!(status, StatusCode::OK, "login begin: {body}");
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: RequestChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();

    let credential: PublicKeyCredential = authenticator
        .do_authentication(origin, challenge)
        .expect("soft passkey authenticates");

    let finish_body = serde_json::json!({
        "credential": credential,
        "state": state,
    });
    let (status, body, headers) = do_request(
        &app,
        "POST",
        "/auth/login/finish",
        Some((finish_body, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login finish: {body}");
    // The body must NOT echo the session id — that hands the bearer to any XSS
    // and defeats the `httpOnly` cookie. Only `user_id` is JS-readable.
    assert!(
        body.get("session_id").is_none(),
        "login response must not leak session_id to JS-readable body: {body}"
    );

    // --- cookie semantics: __Host- prefix, Secure, HttpOnly, SameSite=Strict ---
    let set_cookie = headers.get(SET_COOKIE).unwrap().to_str().unwrap();
    assert!(
        set_cookie.starts_with("__Host-sb_session="),
        "host-prefix: {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly"), "httpOnly: {set_cookie}");
    assert!(set_cookie.contains("Secure"), "secure: {set_cookie}");
    assert!(
        set_cookie.contains("SameSite=Strict"),
        "samesite: {set_cookie}"
    );
    assert!(set_cookie.contains("Path=/"), "path: {set_cookie}");

    // The session id rides only in the cookie; verify it round-trips through
    // `SessionId::parse` (a well-formed ≥256-bit opaque value).
    let session_id = session_id_from_set_cookie(&headers);
    assert!(
        SessionId::parse(&session_id).is_some(),
        "server-side id well-formed"
    );

    let cookie_header = request_cookie_header_value(&SessionId::parse(&session_id).unwrap());

    // --- protected call with the session cookie ---
    let (status, body, _h) = do_request(
        &app,
        "GET",
        "/me",
        Some((Value::Null, Some(cookie_header.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/me with cookie: {body}");
    // `/me` must NOT leak the session id either — only the account id.
    assert!(
        body.get("session_id").is_none(),
        "/me must not leak session_id to JS-readable body: {body}"
    );
    // `user_id` is the stable single-account UUID the passkey bound to.
    assert_eq!(
        body["user_id"].as_str().unwrap(),
        "00000000-0000-0000-0000-000000000001"
    );

    // --- logout ---
    let (status, body, out_headers) = do_request(
        &app,
        "POST",
        "/auth/logout",
        Some((Value::Null, Some(cookie_header.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "logout: {body}");
    // logout must clear the cookie
    let clear = out_headers.get(SET_COOKIE).unwrap().to_str().unwrap();
    assert!(
        clear.contains("__Host-sb_session=") && clear.contains("Max-Age=0"),
        "logout clears cookie: {clear}"
    );

    // --- protected call after logout is rejected (session row gone) ---
    let (status, _body, _h) =
        do_request(&app, "GET", "/me", Some((Value::Null, Some(cookie_header)))).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "stale cookie rejected after logout"
    );
}

#[tokio::test]
async fn protected_route_rejects_missing_cookie() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    // No cookie at all → 401.
    let (status, body, _h) = do_request(&app, "GET", "/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "/me without cookie: {body}"
    );

    // A garbage cookie also → 401 (not 500, not stored).
    let bad = http::HeaderValue::from_static("__Host-sb_session=not-a-real-id");
    let (status, _body, _h) = do_request(&app, "GET", "/me", Some((Value::Null, Some(bad)))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "garbage cookie rejected");
}

#[tokio::test]
async fn login_begin_without_any_registered_passkey_is_404() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));
    let (status, _body, _h) = do_request(&app, "POST", "/auth/login/begin", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recovery_seam_exists_and_is_stubbed() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));
    let (status, body, _h) = do_request(&app, "POST", "/auth/recover", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"].as_str().unwrap(), "recovery_not_implemented");
}
