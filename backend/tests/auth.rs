//! Integration tests for issue #2 + #74: passkey register → login → protected
//! → logout, using a software passkey (`SoftPasskey`) so no hardware is needed.
//!
//! Issue #74 replaces the "first passkey wins" singleton lock with
//! invite-gated registration plus a one-time bootstrap exception. These tests
//! cover: the bootstrap path (zero users → first registration creates the
//! admin, no invite), the invite-gated path (admin mints an invite, invitee
//! registers with it → fresh non-admin user), the error semantics (unknown
//! invite → 404, consumed/reused invite → 410, missing invite once the admin
//! exists → 400), the bootstrap race (two begins, first finish wins, second
//! finish 409), and the unchanged protected/logout behaviour. Login resolves
//! the per-user `user_id` from the authenticated passkey row.

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
const ADMIN_ID: &str = "00000000-0000-0000-0000-000000000001";

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

/// Pull the `Set-Cookie: __Host-sb_session=<id>; ...` value's `<id>` out.
fn session_id_from_set_cookie(headers: &http::HeaderMap) -> String {
    let raw = headers
        .get(SET_COOKIE)
        .expect("login/register finish must set a Set-Cookie")
        .to_str()
        .unwrap();
    let after_eq = raw.split("__Host-sb_session=").nth(1).unwrap();
    let id = after_eq.split(';').next().unwrap();
    id.to_string()
}

/// A truly fresh in-memory DB with ZERO users — the bootstrap-exception
/// precondition. `Db::open_in_memory` seeds the admin under `test-support` for
/// the convenience of the domain tests; the bootstrap path here must start
/// empty, so we open `:memory:` directly.
fn fresh_db() -> Db {
    Db::open(":memory:").expect("fresh in-memory db")
}

/// Drive the begin → soft-passkey → finish registration dance with an optional
/// invite token. Returns (status, body, headers) from the finish call.
async fn register(
    app: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
    invite: Option<&str>,
) -> (StatusCode, Value, http::HeaderMap) {
    let begin_body = serde_json::json!({ "invite": invite });
    let (status, body, _h) = do_request(
        app,
        "POST",
        "/auth/register/begin",
        Some((begin_body, None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "register begin: {body}");
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: CreationChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    let credential = authenticator
        .do_registration(Url::parse(ORIGIN).unwrap(), challenge)
        .expect("soft passkey registers");
    let finish_body = serde_json::json!({ "credential": credential, "state": state });
    do_request(
        app,
        "POST",
        "/auth/register/finish",
        Some((finish_body, None)),
    )
    .await
}

/// Drive the begin → soft-passkey → finish login dance. Returns
/// (status, body, headers).
async fn login(
    app: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) -> (StatusCode, Value, http::HeaderMap) {
    let (status, body, _h) = do_request(app, "POST", "/auth/login/begin", None).await;
    assert_eq!(status, StatusCode::OK, "login begin: {body}");
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: RequestChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    let credential: PublicKeyCredential = authenticator
        .do_authentication(Url::parse(ORIGIN).unwrap(), challenge)
        .expect("soft passkey authenticates");
    let finish_body = serde_json::json!({ "credential": credential, "state": state });
    do_request(app, "POST", "/auth/login/finish", Some((finish_body, None))).await
}

/// Mint an invitation as the admin via the real HTTP endpoint
/// (`POST /admin/invites` with the admin's session cookie). Returns the token.
/// The admin must already be registered (bootstrap) and `admin_cookie` is the
/// session cookie from that registration.
async fn mint_invite(app: &axum::Router, admin_cookie: &http::HeaderValue) -> String {
    let (status, body, _h) = do_request(
        app,
        "POST",
        "/admin/invites",
        Some((Value::Null, Some(admin_cookie.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin mint invite: {body}");
    body["token"]
        .as_str()
        .expect("token field present")
        .to_string()
}

/// Extract the session cookie header value from a Set-Cookie response.
fn cookie_from_headers(headers: &http::HeaderMap) -> http::HeaderValue {
    let session_id = session_id_from_set_cookie(headers);
    request_cookie_header_value(&SessionId::parse(&session_id).unwrap())
}

#[tokio::test]
async fn bootstrap_registration_creates_admin_and_mints_session() {
    // Fresh DB, zero users → the first registration proceeds with no invite and
    // creates the admin (is_admin = true). Registration mints a session and
    // sets the cookie, so /me reads the admin immediately.
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));

    let (status, body, headers) = register(&app, &mut authenticator, None).await;
    assert_eq!(status, StatusCode::OK, "bootstrap register finish: {body}");
    assert_eq!(body["registered"].as_bool(), Some(true));
    let user_id = body["user_id"].as_str().expect("user_id in body");
    assert_eq!(user_id, ADMIN_ID, "bootstrap creates the admin account");

    // The admin row exists with is_admin = 1.
    db.with_conn_test(|conn| {
        let is_admin: i64 = conn.query_row(
            "SELECT is_admin FROM users WHERE id = ?1",
            rusqlite::params![ADMIN_ID],
            |r| r.get(0),
        )?;
        assert_eq!(is_admin, 1, "bootstrap admin must be is_admin=1");
        Ok(())
    })
    .await
    .unwrap();

    // Registration set a session cookie — /me reads the admin without a login.
    let session_id = session_id_from_set_cookie(&headers);
    let cookie = request_cookie_header_value(&SessionId::parse(&session_id).unwrap());
    let (status, body, _h) =
        do_request(&app, "GET", "/me", Some((Value::Null, Some(cookie)))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/me after bootstrap register: {body}"
    );
    assert_eq!(body["user_id"].as_str().unwrap(), ADMIN_ID);
    assert!(body["is_admin"].as_bool().unwrap(), "admin flag is true");
}

#[tokio::test]
async fn bootstrap_exception_closes_once_the_admin_exists() {
    // After the bootstrap admin exists, registration WITHOUT an invite is
    // refused (400 — missing required token), and WITH a valid invite proceeds
    // and creates a fresh non-admin user.
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));
    let mut admin_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));

    // Bootstrap: create the admin.
    let (status, _body, headers) = register(&app, &mut admin_auth, None).await;
    assert_eq!(status, StatusCode::OK, "bootstrap creates admin");
    let admin_cookie = cookie_from_headers(&headers);

    // A second registration with no invite is refused — the exception closed.
    let (status, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": null }), None)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "no invite after admin exists: {body}"
    );

    // Mint an invite and register a second user with it.
    let token = mint_invite(&app, &admin_cookie).await;
    let mut invitee_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (status, body, _h) = register(&app, &mut invitee_auth, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "invitee register finish: {body}");
    let invitee_id = body["user_id"]
        .as_str()
        .expect("invitee user_id")
        .to_string();
    assert_ne!(
        invitee_id, ADMIN_ID,
        "invitee is a fresh non-admin user, not the admin"
    );

    // The invitee is non-admin.
    let invitee_id_for_q = invitee_id.clone();
    db.with_conn_test(move |conn| {
        let is_admin: i64 = conn.query_row(
            "SELECT is_admin FROM users WHERE id = ?1",
            rusqlite::params![invitee_id_for_q],
            |r| r.get(0),
        )?;
        assert_eq!(is_admin, 0, "invitee must be non-admin");
        Ok(())
    })
    .await
    .unwrap();

    // The invite is now consumed.
    let token_for_q = token.clone();
    db.with_conn_test(move |conn| {
        let st: String = conn.query_row(
            "SELECT status FROM invitations WHERE token = ?1",
            rusqlite::params![token_for_q],
            |r| r.get(0),
        )?;
        assert_eq!(st, "consumed", "invite consumed by the invitee");
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unknown_invite_is_404_and_consumed_invite_is_410() {
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));
    let mut admin_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (_, _body, headers) = register(&app, &mut admin_auth, None).await; // bootstrap admin
    let admin_cookie = cookie_from_headers(&headers);

    // Unknown invite → 404 at begin.
    let (status, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": "not-a-real-token" }), None)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown invite: {body}");

    // Mint + consume an invite, then reuse → 410 at begin.
    let token = mint_invite(&app, &admin_cookie).await;
    let mut invitee_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (status, _body, _h) = register(&app, &mut invitee_auth, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "first consume OK");

    let (status, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": token }), None)),
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "reused invite: {body}");
}

#[tokio::test]
async fn reused_invite_at_finish_is_410_atomic_consume() {
    // Two invitees both begin with the same valid invite; the first finish
    // consumes it atomically, the second finish is refused with 410 (the
    // authoritative, race-proof gate under the single-connection mutex).
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));
    let mut admin_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (_, _body, headers) = register(&app, &mut admin_auth, None).await; // bootstrap admin
    let admin_cookie = cookie_from_headers(&headers);

    let token = mint_invite(&app, &admin_cookie).await;

    // Two begins with the same invite — both succeed (begin validates but does
    // not consume).
    let mut a1 = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let mut a2 = WebauthnAuthenticator::new(SoftPasskey::new(true));

    let (st, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": token }), None)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "first begin: {body}");
    let state1 = body["state"].as_str().unwrap().to_string();
    let ch1: CreationChallengeResponse = serde_json::from_value(body["challenge"].clone()).unwrap();
    let cred1 = a1
        .do_registration(Url::parse(ORIGIN).unwrap(), ch1)
        .expect("first registration");

    let (st, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": token }), None)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "second begin: {body}");
    let state2 = body["state"].as_str().unwrap().to_string();
    let ch2: CreationChallengeResponse = serde_json::from_value(body["challenge"].clone()).unwrap();
    let cred2 = a2
        .do_registration(Url::parse(ORIGIN).unwrap(), ch2)
        .expect("second registration");

    // First finish consumes the invite → OK.
    let (st, _body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/finish",
        Some((
            serde_json::json!({ "credential": cred1, "state": state1 }),
            None,
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "first finish consumes invite");

    // Second finish hits the authoritative gate — invite already consumed → 410.
    let (st, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/finish",
        Some((
            serde_json::json!({ "credential": cred2, "state": state2 }),
            None,
        )),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::GONE,
        "second finish must be refused (invite consumed): {body}"
    );
}

#[tokio::test]
async fn bootstrap_race_first_finish_wins_second_is_conflict() {
    // Two begins while zero users exist (both bootstrap); the first finish
    // creates the admin, the second finish is refused because the bootstrap
    // exception closed (re-checked under the mutex).
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));
    let mut a1 = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let mut a2 = WebauthnAuthenticator::new(SoftPasskey::new(true));

    let (_, body1, _) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": null }), None)),
    )
    .await;
    let state1 = body1["state"].as_str().unwrap().to_string();
    let ch1: CreationChallengeResponse =
        serde_json::from_value(body1["challenge"].clone()).unwrap();
    let cred1 = a1
        .do_registration(Url::parse(ORIGIN).unwrap(), ch1)
        .expect("first registration");

    let (_, body2, _) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": null }), None)),
    )
    .await;
    let state2 = body2["state"].as_str().unwrap().to_string();
    let ch2: CreationChallengeResponse =
        serde_json::from_value(body2["challenge"].clone()).unwrap();
    let cred2 = a2
        .do_registration(Url::parse(ORIGIN).unwrap(), ch2)
        .expect("second registration");

    let (st, _body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/finish",
        Some((
            serde_json::json!({ "credential": cred1, "state": state1 }),
            None,
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "first finish creates admin");

    let (st, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/finish",
        Some((
            serde_json::json!({ "credential": cred2, "state": state2 }),
            None,
        )),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "second bootstrap finish refused (exception closed): {body}"
    );
}

#[tokio::test]
async fn login_resolves_per_user_user_id_from_the_passkey() {
    // Bootstrap admin registers, then logs in; the session carries the admin's
    // user_id (resolved from the passkey row, not a constant). Then a second
    // user registers via invite and logs in; its session carries its own id.
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db.clone()));

    let mut admin_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    register(&app, &mut admin_auth, None).await; // bootstrap admin

    // Admin login → session carries ADMIN_ID (resolved from the passkey row).
    let (st, body, headers) = login(&app, &mut admin_auth).await;
    assert_eq!(st, StatusCode::OK, "admin login: {body}");
    assert_eq!(body["user_id"].as_str().unwrap(), ADMIN_ID);
    let admin_cookie = request_cookie_header_value(
        &SessionId::parse(&session_id_from_set_cookie(&headers)).unwrap(),
    );
    let (st, body, _h) = do_request(
        &app,
        "GET",
        "/me",
        Some((Value::Null, Some(admin_cookie.clone()))),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["user_id"].as_str().unwrap(), ADMIN_ID);
    assert!(body["is_admin"].as_bool().unwrap());

    // Invitee registers + logs in → session carries the invitee's id.
    let token = mint_invite(&app, &admin_cookie).await;
    let mut invitee_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (st, body, _h) = register(&app, &mut invitee_auth, Some(&token)).await;
    assert_eq!(st, StatusCode::OK);
    let invitee_id = body["user_id"].as_str().unwrap().to_string();

    let (st, body, headers) = login(&app, &mut invitee_auth).await;
    assert_eq!(st, StatusCode::OK, "invitee login: {body}");
    assert_eq!(
        body["user_id"].as_str().unwrap(),
        invitee_id,
        "invitee session carries the invitee's user_id"
    );
    let invitee_cookie = request_cookie_header_value(
        &SessionId::parse(&session_id_from_set_cookie(&headers)).unwrap(),
    );
    let (st, body, _h) = do_request(
        &app,
        "GET",
        "/me",
        Some((Value::Null, Some(invitee_cookie))),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["user_id"].as_str().unwrap(), invitee_id);
    assert!(!body["is_admin"].as_bool().unwrap(), "invitee is non-admin");
}

#[tokio::test]
async fn full_register_login_protected_logout_flow() {
    let db = fresh_db();
    let app = routes::router(AppState::for_tests(db));
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));

    // --- bootstrap register (zero users → admin, no invite) ---
    let (status, body, headers) = register(&app, &mut authenticator, None).await;
    assert_eq!(status, StatusCode::OK, "register finish: {body}");
    // Registration mints a session and sets the cookie.
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
    let session_id = session_id_from_set_cookie(&headers);
    assert!(
        SessionId::parse(&session_id).is_some(),
        "server-side id well-formed"
    );
    let cookie = request_cookie_header_value(&SessionId::parse(&session_id).unwrap());

    // --- protected call with the session cookie (registration logged us in) ---
    let (status, body, _h) = do_request(
        &app,
        "GET",
        "/me",
        Some((Value::Null, Some(cookie.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "/me with cookie: {body}");
    assert!(
        body.get("session_id").is_none(),
        "/me must not leak session_id: {body}"
    );
    assert_eq!(body["user_id"].as_str().unwrap(), ADMIN_ID);

    // --- login (passkey still works; resolves the admin) ---
    let (status, body, _h) = login(&app, &mut authenticator).await;
    assert_eq!(status, StatusCode::OK, "login finish: {body}");
    assert!(
        body.get("session_id").is_none(),
        "login must not leak session_id: {body}"
    );
    assert_eq!(body["user_id"].as_str().unwrap(), ADMIN_ID);

    // --- logout ---
    let (status, body, out_headers) = do_request(
        &app,
        "POST",
        "/auth/logout",
        Some((Value::Null, Some(cookie.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "logout: {body}");
    let clear = out_headers.get(SET_COOKIE).unwrap().to_str().unwrap();
    assert!(
        clear.contains("__Host-sb_session=") && clear.contains("Max-Age=0"),
        "logout clears cookie: {clear}"
    );

    // --- protected call after logout is rejected ---
    let (status, _body, _h) =
        do_request(&app, "GET", "/me", Some((Value::Null, Some(cookie)))).await;
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

    let (status, body, _h) = do_request(&app, "GET", "/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "/me without cookie: {body}"
    );

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
