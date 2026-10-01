//! Device store and full DPoP flow against a real Postgres.
//!
//! Runs when `SANAD_TEST_DATABASE_URL` points at a disposable database
//! (CI provides a Postgres service; locally: any empty database). Without it,
//! each test prints a skip notice and passes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]

mod common;

use axum::http::StatusCode;
use common::{TestDevice, Tweak, harness_with, register, self_request};
use sanad_kernel::devices::{DeviceStore, NewDevice, StoreError};
use sqlx::postgres::PgPoolOptions;

async fn store() -> Option<DeviceStore> {
    let Ok(url) = std::env::var("SANAD_TEST_DATABASE_URL") else {
        eprintln!("skipping: SANAD_TEST_DATABASE_URL not set");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    Some(DeviceStore::Postgres(pool))
}

fn new_device(dev: &TestDevice) -> NewDevice {
    let pt = sanad_kernel::jose::P256PublicKey::from_jwk(&dev.ecdh_jwk).unwrap();
    NewDevice {
        dpop_jkt: dev.jkt(),
        ecdh_public: pt.uncompressed().to_vec(),
        platform: serde_json::json!({"tier":"A"}),
    }
}

#[tokio::test]
async fn register_is_idempotent_and_revocation_sticks() {
    let Some(store) = store().await else { return };
    let dev = TestDevice::new();
    let a = store.register(new_device(&dev)).await.unwrap();
    let b = store.register(new_device(&dev)).await.unwrap();
    assert_eq!(a.id, b.id, "same key → same device");
    assert_eq!(
        store.get(a.id).await.unwrap().map(|d| d.dpop_jkt),
        Some(dev.jkt())
    );
    assert!(store.revoke(a.id).await.unwrap());
    assert!(
        !store.revoke(a.id).await.unwrap(),
        "second revoke is a no-op"
    );
    assert!(matches!(
        store.register(new_device(&dev)).await,
        Err(StoreError::Revoked)
    ));
}

#[tokio::test]
async fn database_constraints_back_up_the_api_checks() {
    let Some(DeviceStore::Postgres(pool)) = store().await else {
        return;
    };
    // A malformed point or thumbprint can never be stored, even by a buggy caller.
    let bad_point = sqlx::query("INSERT INTO devices (id, dpop_jkt, ecdh_pub) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::now_v7())
        .bind("A".repeat(43))
        .bind(vec![0u8; 64])
        .execute(&pool)
        .await;
    assert!(bad_point.is_err());
    let mut point = vec![4u8];
    point.extend([7u8; 64]);
    let bad_jkt = sqlx::query("INSERT INTO devices (id, dpop_jkt, ecdh_pub) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::now_v7())
        .bind("not a thumbprint")
        .bind(point)
        .execute(&pool)
        .await;
    assert!(bad_jkt.is_err());
}

#[tokio::test]
async fn full_dpop_flow_on_postgres() {
    let Some(store) = store().await else { return };
    let h = harness_with(store);
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["dpop_jkt"], dev.jkt());
}
