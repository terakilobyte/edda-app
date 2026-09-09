//! The published manifest, served over HTTP, must be the one the desktop
//! client's selection logic accepts. No PostgreSQL is needed: the manifest
//! route only reads the artifact directory.

use axum::{body::Body, http::Request};
use ed_api::{
    http::{self, AppState},
    snapshot::community_manifest,
};
use ed_sync::{ArtifactFile, Manifest, ProductKey, MANIFEST_ROUTE};
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn served_manifest_is_selected_by_a_market_only_client() {
    let artifacts = TempDir::new().unwrap();
    let payload = b"not really zstd";
    let artifact = ArtifactFile::for_bytes("community/3/community-3.ebex.zst", payload);
    let published = community_manifest(
        "2026-08-29T12:00:00Z",
        "2026-08-29T11:59:00Z".to_owned(),
        "3",
        artifact.clone(),
    );
    tokio::fs::write(
        artifacts.path().join("current.json"),
        serde_json::to_vec_pretty(&published).unwrap(),
    )
    .await
    .unwrap();

    // The pool is never used by the manifest route; connect lazily.
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .unwrap();
    let response = http::router(AppState::new(pool, artifacts.path().to_owned(), test_metrics()))
        .oneshot(Request::get(MANIFEST_ROUTE).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();

    // Client side: parse, validate, select, verify.
    let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    manifest.validate().unwrap();
    let selected = manifest.community_baseline().unwrap();
    assert_eq!(selected.key, ProductKey::Community);
    assert_eq!(selected.product.version, "3");
    assert_eq!(selected.artifact, &artifact);
    assert_eq!(
        selected.artifact.route(),
        "/v1/artifacts/community/3/community-3.ebex.zst"
    );
    selected.artifact.verify_reader(&payload[..]).unwrap();
}

/// A detached recorder per call: tests must not fight over the one
/// global recorder slot.
fn test_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder().handle()
}
