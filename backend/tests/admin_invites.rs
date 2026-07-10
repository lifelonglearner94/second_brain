//! Integration test for issue #73: admin invite minting.
//!
//! The bootstrap admin mints a single-use invitation (POST /admin/invites →
//! 200) and lists all invitations (GET /admin/invites → 200, the minted token
//! appears). A second, non-admin user's session is refused with 403 on both
//! minting and listing. The admin guard reads `SessionInfo.is_admin` (sourced
//! from the `users` table by `lookup_session`), so the bootstrap admin can mint
//! immediately and a plain user cannot.

use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::{routes, state::AppState};
use serde_json::Value;
use tower::ServiceExt;

const ADMIN_ID: &str = "00000000-0000-0000-0000-000000000001";
const USER_B_ID: &str = "00000000-0000-0000-0000-000000000002";

/// Mint a session for `user_id` and return the cookie header value.
async fn session_cookie(db: &Db, user_id: &str) -> http::HeaderValue {
    let session = mint_session(db, user_id).await.unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

/// POST /admin/invites with the given cookie. Returns (status, body).
async fn mint_invite(app: &axum::Router, cookie: &http::HeaderValue) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/admin/invites")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// GET /admin/invites with the given cookie. Returns (status, body).
async fn list_invites(app: &axum::Router, cookie: &http::HeaderValue) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri("/admin/invites")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Build an app backed by an in-memory DB with the bootstrap admin already
/// present (the migration seeds it). Invites don't touch the graph or LLM, so
/// the default test state is sufficient.
fn app() -> (axum::Router, Db) {
    let db = Db::open_in_memory().unwrap();
    let state = AppState::for_tests(db.clone());
    (routes::router(state), db)
}

#[tokio::test]
async fn admin_mints_and_lists_invite() {
    let (app, db) = app();

    // Create a second, non-admin user so we can prove the guard refuses them.
    db.with_conn_test(|conn| {
        conn.execute(
            "INSERT INTO users (id, display_name, is_admin, created_at)
             VALUES (?1, 'user_b', 0, unixepoch())",
            rusqlite::params![USER_B_ID],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let cookie_admin = session_cookie(&db, ADMIN_ID).await;
    let cookie_b = session_cookie(&db, USER_B_ID).await;

    // The admin mints an invite → 200 with a non-empty token and pending status.
    let (status, body) = mint_invite(&app, &cookie_admin).await;
    assert_eq!(status, StatusCode::OK, "admin mint: {body}");
    let token = body["token"].as_str().expect("token field present");
    assert!(!token.is_empty(), "minted token must be non-empty");
    assert_eq!(body["status"].as_str(), Some("pending"));
    assert_eq!(body["created_by_user_id"].as_str(), Some(ADMIN_ID));
    let invite_id = body["id"].as_i64().expect("id field present");

    // A second mint produces a distinct token (no reuse).
    let (status, body2) = mint_invite(&app, &cookie_admin).await;
    assert_eq!(status, StatusCode::OK, "admin second mint: {body2}");
    let token2 = body2["token"].as_str().expect("second token present");
    assert_ne!(token, token2, "two mints must produce distinct tokens");

    // The admin lists invites → 200; both mints appear, including the first
    // token (so the admin can re-share a pending invite out-of-band).
    let (status, list) = list_invites(&app, &cookie_admin).await;
    assert_eq!(status, StatusCode::OK, "admin list: {list}");
    let invites = list["invitations"]
        .as_array()
        .expect("invitations is a list");
    assert_eq!(
        invites.len(),
        2,
        "admin sees both minted invites: {invites:?}"
    );
    let tokens: Vec<&str> = invites
        .iter()
        .map(|i| i["token"].as_str().unwrap())
        .collect();
    assert!(
        tokens.contains(&token),
        "first minted token appears in list"
    );
    assert!(
        tokens.contains(&token2),
        "second minted token appears in list"
    );
    // Each row carries the creator and pending status with no consumer yet.
    let first = invites
        .iter()
        .find(|i| i["token"].as_str() == Some(token))
        .unwrap();
    assert_eq!(first["id"].as_i64(), Some(invite_id));
    assert_eq!(first["status"].as_str(), Some("pending"));
    assert_eq!(first["created_by_user_id"].as_str(), Some(ADMIN_ID));
    assert!(first["consumed_at"].is_null());
    assert!(first["consumed_by_user_id"].is_null());

    // A non-admin session is refused on mint → 403.
    let (status, body) = mint_invite(&app, &cookie_b).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin mint must be refused: {body}"
    );

    // A non-admin session is refused on list → 403.
    let (status, body) = list_invites(&app, &cookie_b).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin list must be refused: {body}"
    );

    // The non-admin 403s did not mint anything - the admin's list is unchanged.
    let (status, list) = list_invites(&app, &cookie_admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list["invitations"].as_array().unwrap().len(),
        2,
        "non-admin 403s must not create invites"
    );
}

#[tokio::test]
async fn unauthenticated_request_is_unauthorized_not_forbidden() {
    // No session cookie at all → 401 (the session guard runs before the admin
    // guard), not 403. Proves the admin guard is layered behind require_session.
    let (app, _db) = app();

    let (status, _body) = mint_invite(&app, &http::HeaderValue::from_static("")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let request = Request::builder()
        .uri("/admin/invites")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn minted_token_is_unguessable_bearer_shape() {
    let (app, db) = app();
    let cookie_admin = session_cookie(&db, ADMIN_ID).await;

    // Mint a few tokens and assert they are base64url (no padding), reasonably
    // long, and mutually distinct - a sanity check on the CSPRNG minter.
    let mut tokens = Vec::new();
    for _ in 0..4 {
        let (status, body) = mint_invite(&app, &cookie_admin).await;
        assert_eq!(status, StatusCode::OK);
        let token = body["token"].as_str().unwrap().to_string();
        assert!(
            token.len() >= 32,
            "token has ≥256 bits of entropy encoded: {token}"
        );
        assert!(
            !token.contains('=') && !token.contains('+') && !token.contains('/'),
            "token is base64url (no padding, url-safe): {token}"
        );
        assert!(!tokens.contains(&token), "tokens are distinct: {tokens:?}");
        tokens.push(token);
    }
}
