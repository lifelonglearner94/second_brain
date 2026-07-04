//! Integration test for the skeleton: `GET /health` reports DB + sqlite-vec.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::{db::Db, routes, state::AppState};
use tower::ServiceExt;

#[tokio::test]
async fn health_reports_db_and_sqlite_vec_ok() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["db"], true);
    assert_eq!(value["sqlite_vec"], true);
}
