//! End-to-end integration test for issue #74: the invite-gated registration
//! chain, closing the #72 → #73 → #74 loop.
//!
//! Flow under test: the bootstrap admin registers (no invite, zero users) →
//! the admin mints a single-use invitation → an invitee registers with the
//! token (a fresh non-admin `users` row is created, the invite is consumed) →
//! the invitee logs in (the session carries the invitee's per-user `user_id`,
//! resolved from the passkey row) → the invitee submits a Braindump → the
//! admin's Global Topology Snapshot excludes it (per-user isolation from #72
//! holds) → reusing the consumed invite for a second registration is refused
//! (410).

use std::sync::Arc;

use axum::body::Body;
use http::header::{COOKIE, SET_COOKIE};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::session::SessionId;
use second_brain_backend::db::Db;
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::graph_repo::SqliteGraphRepo;
use second_brain_backend::llm::Llm;
use second_brain_backend::{routes, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;
use webauthn_authenticator_rs::prelude::{Url, WebauthnAuthenticator};
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs_proto::{CreationChallengeResponse, RequestChallengeResponse};

const ORIGIN: &str = "http://localhost:8080";

/// A truly fresh in-memory DB with ZERO users - the bootstrap-exception
/// precondition. `Db::open_in_memory` seeds the admin under `test-support` for
/// the convenience of the domain tests; the bootstrap path here must start
/// empty.
fn fresh_db() -> Db {
    Db::open(":memory:").expect("fresh in-memory db")
}

/// An LLM that extracts a fixed concept+edge set from any braindump and
/// produces deterministic embeddings, mirroring `per_user_isolation.rs`. The
/// invitee's braindump yields graph data we can assert the admin does NOT see.
struct ScriptedLlm {
    result: ExtractionResult,
}

#[async_trait::async_trait]
impl Llm for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> second_brain_backend::error::Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(
        &self,
        _: &str,
        user: &str,
    ) -> second_brain_backend::error::Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _: &str, _: &str) -> second_brain_backend::error::Result<String> {
        Ok("synthesized answer".to_string())
    }
    async fn extract(
        &self,
        _: &str,
        _: &[String],
    ) -> second_brain_backend::error::Result<ExtractionResult> {
        Ok(self.result.clone())
    }
    async fn embed_document(&self, text: &str) -> second_brain_backend::error::Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    async fn embed_query(&self, text: &str) -> second_brain_backend::error::Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    fn dim(&self) -> usize {
        64
    }
}

fn extraction(concepts: &[&str], edges: &[(&str, &str, &str)]) -> ExtractionResult {
    ExtractionResult {
        concepts: concepts
            .iter()
            .map(|l| ExtractedConcept {
                label: l.to_string(),
            })
            .collect(),
        edges: edges
            .iter()
            .map(|(s, t, tg)| ExtractedEdge {
                from_label: s.to_string(),
                type_slug: t.to_string(),
                to_label: tg.to_string(),
            })
            .collect(),
    }
}

/// Build AppState with a scripted LLM that extracts the given concepts+edges.
fn app_with_extraction(result: ExtractionResult) -> (axum::Router, Db) {
    let db = fresh_db();
    let llm: Arc<dyn Llm> = Arc::new(ScriptedLlm { result });
    db.ensure_vec_tables(llm.dim()).unwrap();
    let webauthn =
        second_brain_backend::auth::build_webauthn("localhost", "http://localhost:8080", "test")
            .unwrap();
    let graph_repo: Arc<dyn second_brain_backend::graph_repo::GraphRepo> =
        Arc::new(SqliteGraphRepo::new(db.clone()));
    let state = AppState {
        db: db.clone(),
        config: Arc::new(second_brain_backend::config::Config::for_tests()),
        llm,
        auth: second_brain_backend::auth::AuthService::new(webauthn),
        log_buffer: second_brain_backend::logs::LogBuffer::with_default_capacity(),
        refactor_runner: second_brain_backend::ontology::RefactorRunner::new(),
        ingest_runner: second_brain_backend::braindump::IngestRunner::new_inline(),
        graph_repo,
    };
    (routes::router(state), db)
}

/// Pull the session id out of a `Set-Cookie` header and wrap it as a
/// `Cookie: __Host-sb_session=<id>` request header value.
fn cookie_from_headers(headers: &http::HeaderMap) -> http::HeaderValue {
    let raw = headers
        .get(SET_COOKIE)
        .expect("register/login finish must set a Set-Cookie")
        .to_str()
        .unwrap();
    let after_eq = raw.split("__Host-sb_session=").nth(1).unwrap();
    let id = after_eq.split(';').next().unwrap();
    request_cookie_header_value(&SessionId::parse(id).expect("well-formed session id"))
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

/// Drive the begin → soft-passkey → finish login dance. Returns the cookie
/// header value for the resulting session.
async fn login(
    app: &axum::Router,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) -> http::HeaderValue {
    let (status, body, _h) = do_request(app, "POST", "/auth/login/begin", None).await;
    assert_eq!(status, StatusCode::OK, "login begin: {body}");
    let state = body["state"].as_str().unwrap().to_string();
    let challenge: RequestChallengeResponse =
        serde_json::from_value(body["challenge"].clone()).unwrap();
    let credential = authenticator
        .do_authentication(Url::parse(ORIGIN).unwrap(), challenge)
        .expect("soft passkey authenticates");
    let finish_body = serde_json::json!({ "credential": credential, "state": state });
    let (status, body, headers) =
        do_request(app, "POST", "/auth/login/finish", Some((finish_body, None))).await;
    assert_eq!(status, StatusCode::OK, "login finish: {body}");
    cookie_from_headers(&headers)
}

/// POST /admin/invites with the admin cookie → the new token.
async fn mint_invite(app: &axum::Router, admin_cookie: &http::HeaderValue) -> String {
    let (status, body, _h) = do_request(
        app,
        "POST",
        "/admin/invites",
        Some((Value::Null, Some(admin_cookie.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "mint invite: {body}");
    body["token"].as_str().expect("token present").to_string()
}

/// Submit a braindump via POST /braindumps with the given cookie.
async fn submit_braindump(app: &axum::Router, cookie: &http::HeaderValue, verbatim: &str) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/braindumps")
        .header(COOKIE, cookie)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "verbatim": verbatim }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "submit braindump: {value}");
    value
}

/// GET /graph (topology snapshot) - decompress gzip and parse JSON.
async fn get_graph(app: &axum::Router, cookie: &http::HeaderValue) -> Value {
    let request = Request::builder()
        .uri("/graph")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut json_str = String::new();
    decoder.read_to_string(&mut json_str).unwrap();
    serde_json::from_str(&json_str).unwrap()
}

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

#[tokio::test]
async fn invite_chain_bootstrap_admin_mints_invitee_registers_logins_submits_admin_snapshot_excludes_reuse_refused(
) {
    // Script the LLM to extract one concept + one edge from any braindump, so
    // the invitee's submission produces graph data the admin must NOT see.
    let (app, db) = app_with_extraction(extraction(
        &["Solo Project"],
        &[("Solo Project", "endangers", "Solo Project")],
    ));

    // --- 1. Bootstrap: zero users → first registration creates the admin ---
    let mut admin_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (status, body, headers) = register(&app, &mut admin_auth, None).await;
    assert_eq!(status, StatusCode::OK, "bootstrap register: {body}");
    assert_eq!(
        body["user_id"].as_str().unwrap(),
        "00000000-0000-0000-0000-000000000001",
        "bootstrap creates the admin"
    );
    let admin_cookie = cookie_from_headers(&headers);

    // The admin is is_admin = 1 and the bootstrap exception is now closed.
    db.with_conn_test(|conn| {
        let (count, admin_is_admin): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), (SELECT is_admin FROM users WHERE id = '00000000-0000-0000-0000-000000000001') FROM users",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(count, 1, "exactly one user (the admin) after bootstrap");
        assert_eq!(admin_is_admin, 1);
        Ok(())
    })
    .await
    .unwrap();

    // --- 2. Admin mints a single-use invitation ---
    let token = mint_invite(&app, &admin_cookie).await;
    assert!(!token.is_empty());

    // --- 3. Invitee registers with the token → fresh non-admin user ---
    let mut invitee_auth = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (status, body, headers) = register(&app, &mut invitee_auth, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "invitee register: {body}");
    let invitee_id = body["user_id"].as_str().unwrap().to_string();
    assert_ne!(
        invitee_id, "00000000-0000-0000-0000-000000000001",
        "invitee is a fresh non-admin user"
    );
    let invitee_cookie = cookie_from_headers(&headers);

    // The invite is now consumed by the invitee; the invitee is non-admin.
    let invitee_id_for_q = invitee_id.clone();
    let invitee_id_expected = invitee_id.clone();
    let token_for_q = token.clone();
    db.with_conn_test(move |conn| {
        let (is_admin, invite_status, consumed_by): (i64, String, Option<String>) = conn
            .query_row(
                "SELECT u.is_admin, i.status, i.consumed_by_user_id
             FROM users u, invitations i
             WHERE u.id = ?1 AND i.token = ?2",
                rusqlite::params![invitee_id_for_q, token_for_q],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        assert_eq!(is_admin, 0, "invitee is non-admin");
        assert_eq!(invite_status, "consumed", "invite is consumed");
        assert_eq!(
            consumed_by.as_deref(),
            Some(invitee_id_expected.as_str()),
            "invite recorded the invitee as the consumer"
        );
        Ok(())
    })
    .await
    .unwrap();

    // --- 4. Invitee logs in → session carries the invitee's user_id ---
    let invitee_login_cookie = login(&app, &mut invitee_auth).await;
    let (status, me, _h) = do_request(
        &app,
        "GET",
        "/me",
        Some((Value::Null, Some(invitee_login_cookie.clone()))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "invitee /me: {me}");
    assert_eq!(me["user_id"].as_str().unwrap(), invitee_id);
    assert!(
        !me["is_admin"].as_bool().unwrap(),
        "invitee /me is non-admin"
    );

    // --- 5. Invitee submits a Braindump → extraction produces a concept ---
    let bd = submit_braindump(&app, &invitee_cookie, "solo project endangers solo project").await;
    let bd_id = bd["id"].as_i64().unwrap();
    assert!(bd_id > 0, "invitee braindump persisted");

    // --- 6. Per-user isolation: the admin's Global Topology Snapshot
    //     EXCLUDES the invitee's concept/braindump. The admin has submitted
    //     nothing, so the admin's graph is empty; the invitee's graph has the
    //     extracted concept. ---
    let admin_graph = get_graph(&app, &admin_cookie).await;
    let admin_concepts = admin_graph["concepts"].as_array().unwrap();
    assert!(
        admin_concepts.is_empty(),
        "admin snapshot excludes the invitee's concepts: {admin_concepts:?}"
    );
    assert!(
        admin_graph["edges"].as_array().unwrap().is_empty(),
        "admin snapshot excludes the invitee's edges"
    );

    let invitee_graph = get_graph(&app, &invitee_cookie).await;
    let invitee_concepts = invitee_graph["concepts"].as_array().unwrap();
    assert_eq!(
        invitee_concepts.len(),
        1,
        "invitee sees exactly their own concept: {invitee_concepts:?}"
    );
    assert_eq!(
        invitee_concepts[0]["label"].as_str().unwrap(),
        "Solo Project"
    );

    // The admin cannot read the invitee's braindump by id (cross-user 404).
    let (status, _body, _h) = do_request(
        &app,
        "GET",
        &format!("/braindumps/{bd_id}"),
        Some((Value::Null, Some(admin_cookie))),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "admin cannot read the invitee's braindump by id"
    );

    // --- 7. Reusing the consumed invite for a second registration is refused
    //     (410 at begin - the invite is already consumed). ---
    let (status, body, _h) = do_request(
        &app,
        "POST",
        "/auth/register/begin",
        Some((serde_json::json!({ "invite": token }), None)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::GONE,
        "reusing a consumed invite is refused (410): {body}"
    );

    // And a second registration with NO invite is also refused (the admin
    // exists, so the bootstrap exception is closed → 400 missing invite).
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
        "no invite after admin exists is refused (400): {body}"
    );
}
