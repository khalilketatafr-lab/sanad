//! Stub lease end to end through the router: a registered device opens the
//! canary edition and unwraps the chunk keys exactly as the Vault Worker
//! does (ECDH → HKDF → AES-KW), getting Folio's chunk keys.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aws_lc_rs::agreement::{ECDH_P256, UnparsedPublicKey, agree};
use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use aws_lc_rs::key_wrap::{AES_256, AesKek, KeyWrap};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Harness, TestDevice, Tweak, harness, register, register_member};
use sanad_folio::seal::{TitleMasterKey, derive_chunk_key};
use sanad_kernel::catalog::CANARY_EDITION;
use sanad_kernel::jose::{P256PublicKey, b64url_decode};
use sanad_kernel::lease::LEASE_INFO;
use serde_json::{Value, json};
use uuid::Uuid;

fn open_request(path: &str, proof: &str, token: &str, body: &Value) -> Request<Body> {
    Request::post(path)
        .header("authorization", format!("DPoP {token}"))
        .header("dpop", proof)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn open(
    h: &Harness,
    dev: &TestDevice,
    token: &str,
    path: &str,
    body: &Value,
) -> common::Response {
    let proof = dev.proof(
        "POST",
        path,
        Some(&h.nonce()),
        Some(token),
        &Tweak::default(),
    );
    h.send(open_request(path, &proof, token, body)).await
}

/// The Vault Worker's unwrap, in Rust.
fn unwrap_key(dev: &TestDevice, lease: &Value, wrapped_b64: &str) -> Vec<u8> {
    let eph = P256PublicKey::from_jwk(&lease["ephemeral_public_jwk"]).unwrap();
    let lease_id = Uuid::parse_str(lease["lease_id"].as_str().unwrap()).unwrap();
    let kek = agree(
        &dev.ecdh,
        UnparsedPublicKey::new(&ECDH_P256, eph.uncompressed()),
        (),
        |z| {
            let mut kek = [0u8; 32];
            Salt::new(HKDF_SHA256, lease_id.as_bytes())
                .extract(z)
                .expand(&[LEASE_INFO], HKDF_SHA256)
                .unwrap()
                .fill(&mut kek)
                .unwrap();
            Ok::<_, ()>(kek)
        },
    )
    .unwrap();
    let wrapped = b64url_decode(wrapped_b64).unwrap();
    let mut out = [0u8; 32];
    AesKek::new(&AES_256, &kek)
        .unwrap()
        .unwrap(&wrapped, &mut out)
        .unwrap();
    out.to_vec()
}

fn canary_path() -> String {
    format!("/kernel/v1/editions/{CANARY_EDITION}:open")
}

#[tokio::test]
async fn device_opens_the_canary_edition_and_unwraps_folio_chunk_keys() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let res = open(&h, &dev, &token, &canary_path(), &json!({})).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.headers["cache-control"], "no-store");
    let lease = &res.body;
    assert_eq!(lease["edition_id"], CANARY_EDITION.to_string());
    assert_eq!(lease["window"], json!([0, 3]));
    assert_eq!(lease["expires_in"], 900);

    let tmk = TitleMasterKey::from_bytes([0x42; 32]);
    let keys = lease["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 3);
    for k in keys {
        let chunk = u32::try_from(k["chunk"].as_u64().unwrap()).unwrap();
        let expected = derive_chunk_key(&tmk, CANARY_EDITION.as_bytes(), 0, chunk).unwrap();
        assert_eq!(
            unwrap_key(&dev, lease, k["wrapped"].as_str().unwrap()),
            expected.expose_for_wrapping()
        );
    }

    let rct = h
        .kernel
        .tokens
        .verify_rct(lease["rct"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        rct.jkt,
        dev.jkt(),
        "the RCT is bound to the device's DPoP key"
    );
    assert_eq!(rct.window, [0, 3]);
    assert_eq!(rct.lease_id.to_string(), lease["lease_id"]);
}

#[tokio::test]
async fn window_start_and_end_follow_the_request() {
    let h = harness();
    let dev = TestDevice::new();
    // A signed-in member opens the free edition in full.
    let token = register_member(&h, &dev).await;
    let res = open(&h, &dev, &token, &canary_path(), &json!({ "start": 4 })).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["window"], json!([4, 6]), "clipped to the 6 chunks");
    let res = open(&h, &dev, &token, &canary_path(), &json!({ "start": 6 })).await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn anonymous_sampling_is_limited_to_the_first_chapters() {
    let h = harness();
    let dev = TestDevice::new();
    // A device with no account samples the free edition.
    let token = register(&h, &dev).await;
    // From the start: the window is the sample (first SAMPLE_CHAPTERS chunks).
    let res = open(&h, &dev, &token, &canary_path(), &json!({ "start": 0 })).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["window"], json!([0, 3]));
    // Mid-sample: the window never bleeds past the sample boundary.
    let res = open(&h, &dev, &token, &canary_path(), &json!({ "start": 2 })).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.body["window"],
        json!([2, 3]),
        "clamped to the sample ceiling"
    );
    // Past the sample window: denied until the reader signs in.
    let res = open(&h, &dev, &token, &canary_path(), &json!({ "start": 3 })).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    // The same chapter opens for a signed-in member.
    let member = TestDevice::new();
    let mtoken = register_member(&h, &member).await;
    let res = open(&h, &member, &mtoken, &canary_path(), &json!({ "start": 3 })).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
}

#[tokio::test]
async fn another_device_cannot_use_the_wrapped_keys() {
    let h = harness();
    let (a, b) = (TestDevice::new(), TestDevice::new());
    let token = register(&h, &a).await;
    register(&h, &b).await;
    let res = open(&h, &a, &token, &canary_path(), &json!({})).await;
    let lease = &res.body;
    let wrapped = lease["keys"][0]["wrapped"].as_str().unwrap();
    let result = std::panic::catch_unwind(|| unwrap_key(&b, lease, wrapped));
    assert!(
        result.is_err(),
        "device B's ECDH key must not unwrap device A's lease"
    );
}

#[tokio::test]
async fn unknown_editions_operations_and_unauthenticated_calls_are_refused() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let unknown = format!("/kernel/v1/editions/{}:open", Uuid::now_v7());
    assert_eq!(
        open(&h, &dev, &token, &unknown, &json!({})).await.status,
        StatusCode::NOT_FOUND
    );
    let bad_op = format!("/kernel/v1/editions/{CANARY_EDITION}:delete");
    assert_eq!(
        open(&h, &dev, &token, &bad_op, &json!({})).await.status,
        StatusCode::NOT_FOUND
    );
    let res = h
        .send(
            Request::post(canary_path())
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(
        res.status,
        StatusCode::UNAUTHORIZED,
        "no lease without a DPoP-bound token"
    );
}
