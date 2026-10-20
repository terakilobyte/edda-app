//! The published manifest, served over HTTP, must be one the desktop client
//! accepts. No PostgreSQL is needed: the manifest route only reads the
//! artifact directory.

use axum::{body::Body, http::Request};
use ed_api::http::{self, AppState};
use ed_sync::{ArtifactFile, Manifest, Product, ProductKey, MANIFEST_ROUTE};
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use tempfile::TempDir;
use tower::ServiceExt;

/// The 2026-10-20 retirement of the `community` and `market_daily`
/// products: a `current.json` that still names them (the box's, until the
/// one-off `deploy/retire-market-products.sh` ran) is served as it is, and
/// a reader sees only the products that still exist.
#[tokio::test]
async fn served_manifest_skips_retired_products() {
    let artifacts = TempDir::new().unwrap();
    let payload = b"not really zstd";
    let stars = ArtifactFile::for_bytes("stars/3/stars-3.ebex.zst", payload);
    let published = Manifest::with_product(
        None,
        "2026-08-29T12:00:00Z",
        Some("2026-08-29T11:59:00Z".to_owned()),
        ProductKey::Stars,
        Product {
            version: "3".to_owned(),
            schema: ed_sync::STARS_SCHEMA_V1,
            minimum_client: None,
            files: vec![stars.clone()],
            overlays: Vec::new(),
        },
    );
    let mut json = serde_json::to_value(&published).unwrap();
    json["products"]["community"] = serde_json::json!({
        "version": "4703de92", "schema": 1,
        "files": [{"path": "community/4703de92/community-4703de92.ebex.zst", "bytes": 692785616, "sha256": "0".repeat(64)}]
    });
    json["products"]["market_daily"] = serde_json::json!({
        "version": "9435b656", "schema": 1, "covers_from": 1760000000,
        "files": [{"path": "market_daily/9435b656/market-daily-9435b656.ebex.zst", "bytes": 59396789, "sha256": "0".repeat(64)}]
    });
    tokio::fs::write(
        artifacts.path().join("current.json"),
        serde_json::to_vec_pretty(&json).unwrap(),
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

    let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    manifest.validate().unwrap();
    assert_eq!(
        manifest.products.keys().copied().collect::<Vec<_>>(),
        [ProductKey::Stars],
        "retired products are unknown keys, skipped"
    );
    let product = &manifest.products[&ProductKey::Stars];
    assert_eq!(product.version, "3");
    assert_eq!(product.files[0], stars);
    assert_eq!(product.files[0].route(), "/v1/artifacts/stars/3/stars-3.ebex.zst");
    product.files[0].verify_reader(&payload[..]).unwrap();
}

/// A detached recorder per call: tests must not fight over the one
/// global recorder slot.
fn test_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder().handle()
}
