//! Integration tests for issue #3: `GET /ontology` returns the seeded
//! edge-type vocabulary, read-only. Issue #72: `/ontology` is now behind the
//! auth middleware (the ontology is per-user), so the tests mint a session.

use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::{db::Db, routes, state::AppState};
use tower::ServiceExt;

const EXPECTED_SEED_SLUGS: &[&str] = &[
    "relates_to",
    "causes",
    "affects",
    "endangers",
    "helps",
    "part_of",
    "depends_on",
    "supports",
    "contradicts",
    "precedes",
    "enables",
    "produces",
    "derived_from",
];

/// Mint a session for the bootstrap admin and return the cookie header value.
async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

#[tokio::test]
async fn get_ontology_returns_seeded_edge_types() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db.clone()));
    let cookie = session_cookie(&db).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/ontology")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let types = value
        .get("edge_types")
        .and_then(|v| v.as_array())
        .expect("body has an `edge_types` array");
    assert!(!types.is_empty(), "ontology must be seeded");

    let slugs: Vec<&str> = types
        .iter()
        .map(|t| t["slug"].as_str().expect("slug is a string"))
        .collect();
    let mut sorted = slugs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(slugs.len(), sorted.len(), "slugs must be unique: {slugs:?}");

    for slug in EXPECTED_SEED_SLUGS {
        assert!(
            slugs.contains(slug),
            "missing seed slug `{slug}`: {slugs:?}"
        );
    }
    for t in types {
        let slug = t["slug"].as_str().unwrap();
        assert!(
            !t["label"].as_str().unwrap().is_empty(),
            "label for `{slug}`"
        );
        assert!(
            !t["description"].as_str().unwrap().is_empty(),
            "description for `{slug}`"
        );
    }

    let causes = types
        .iter()
        .find(|t| t["slug"] == "causes")
        .expect("causes is seeded");
    assert_eq!(causes["label"], "Causes");
    assert_eq!(
        causes["description"],
        "A brings about B; B would not have occurred without A."
    );
}

#[tokio::test]
async fn get_ontology_is_get_only() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db.clone()));
    let cookie = session_cookie(&db).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ontology")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
