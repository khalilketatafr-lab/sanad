//! Shared test client: a Rust stand-in for the browser's Vault Worker
//! (ES256 over P-256, fixed-width r‖s signatures, JWK in the proof header).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    unreachable_pub
)]

use std::sync::Arc;

use aws_lc_rs::agreement::{ECDH_P256, PrivateKey};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use sanad_kernel::config::KernelConfig;
use sanad_kernel::devices::DeviceStore;
use sanad_kernel::jose::{P256PublicKey, access_token_hash, b64url};
use sanad_kernel::{Kernel, router, unix_now};
use serde_json::{Value, json};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

pub mod authenticator;

pub const ORIGIN: &str = "https://kernel.sanad.test";

pub struct Harness {
    pub kernel: Arc<Kernel>,
    pub app: Router,
}

pub fn harness() -> Harness {
    harness_with(DeviceStore::memory())
}

pub fn harness_with(store: DeviceStore) -> Harness {
    let mut config = KernelConfig::development(Url::parse(ORIGIN).unwrap());
    config.reader_origins = vec![authenticator::READER.into()];
    config.rp_id = authenticator::RP_ID.into();
    let kernel = Arc::new(Kernel::new(config, store).unwrap());
    Harness {
        app: router(kernel.clone()),
        kernel,
    }
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl Harness {
    pub async fn send(&self, req: Request<Body>) -> Response {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Response {
            status,
            headers,
            body,
        }
    }

    pub fn nonce(&self) -> String {
        self.kernel.nonces.issue(unix_now())
    }
}

pub fn point_jwk(uncompressed: &[u8]) -> Value {
    assert_eq!(uncompressed.len(), 65);
    json!({ "kty": "EC", "crv": "P-256", "x": b64url(&uncompressed[1..33]), "y": b64url(&uncompressed[33..]), "ext": true, "key_ops": [] })
}

/// A browser-equivalent device: DeviceKey-Sign + DeviceKey-ECDH.
pub struct TestDevice {
    pub sign: EcdsaKeyPair,
    pub jwk: Value,
    pub ecdh: PrivateKey,
    pub ecdh_jwk: Value,
}

#[derive(Default)]
pub struct Tweak {
    pub header: Option<fn(&mut Value)>,
    pub claims: Option<fn(&mut Value)>,
}

impl TestDevice {
    pub fn new() -> Self {
        let sign = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
        let jwk = point_jwk(sign.public_key().as_ref());
        let ecdh = PrivateKey::generate(&ECDH_P256).unwrap();
        let ecdh_jwk = point_jwk(ecdh.compute_public_key().unwrap().as_ref());
        Self {
            sign,
            jwk,
            ecdh,
            ecdh_jwk,
        }
    }

    pub fn jkt(&self) -> String {
        P256PublicKey::from_jwk(&self.jwk).unwrap().thumbprint()
    }

    pub fn proof(
        &self,
        method: &str,
        path: &str,
        nonce: Option<&str>,
        token: Option<&str>,
        tweak: &Tweak,
    ) -> String {
        let mut header = json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": self.jwk });
        let mut claims = json!({
            "jti": b64url(Uuid::new_v4().as_bytes()),
            "htm": method,
            "htu": format!("{ORIGIN}{path}"),
            "iat": unix_now(),
        });
        if let Some(n) = nonce {
            claims["nonce"] = n.into();
        }
        if let Some(t) = token {
            claims["ath"] = access_token_hash(t).into();
        }
        if let Some(f) = tweak.header {
            f(&mut header);
        }
        if let Some(f) = tweak.claims {
            f(&mut claims);
        }
        let input = format!(
            "{}.{}",
            b64url(header.to_string().as_bytes()),
            b64url(claims.to_string().as_bytes())
        );
        let sig = self
            .sign
            .sign(&SystemRandom::new(), input.as_bytes())
            .unwrap();
        format!("{input}.{}", b64url(sig.as_ref()))
    }
}

pub fn register_request(proof: &str, body: &Value) -> Request<Body> {
    Request::post("/kernel/v1/devices")
        .header("content-type", "application/json")
        .header("dpop", proof)
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub fn self_request(proof: &str, token: &str, scheme: &str) -> Request<Body> {
    Request::get("/kernel/v1/devices/self")
        .header("authorization", format!("{scheme} {token}"))
        .header("dpop", proof)
        .body(Body::empty())
        .unwrap()
}

/// Registers `dev` (following the nonce challenge) and returns the access token.
pub async fn register(h: &Harness, dev: &TestDevice) -> String {
    let body = json!({ "ecdh_public_jwk": dev.ecdh_jwk, "platform": { "tier": "B" } });
    let first = h
        .send(register_request(
            &dev.proof("POST", "/kernel/v1/devices", None, None, &Tweak::default()),
            &body,
        ))
        .await;
    assert_eq!(first.status, StatusCode::BAD_REQUEST);
    assert_eq!(first.body["error"], "use_dpop_nonce");
    let nonce = first
        .headers
        .get("dpop-nonce")
        .expect("nonce challenge")
        .to_str()
        .unwrap()
        .to_owned();

    let ok = h
        .send(register_request(
            &dev.proof(
                "POST",
                "/kernel/v1/devices",
                Some(&nonce),
                None,
                &Tweak::default(),
            ),
            &body,
        ))
        .await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.body);
    assert_eq!(ok.body["token_type"], "DPoP");
    assert_eq!(ok.body["dpop_jkt"], dev.jkt());
    assert!(ok.headers.contains_key("dpop-nonce"));
    ok.body["access_token"].as_str().unwrap().to_owned()
}

/// Registers `dev` and attaches a user account to it, so it opens free
/// editions in full (not just the anonymous sample). Returns the access token.
pub async fn register_member(h: &Harness, dev: &TestDevice) -> String {
    let body = json!({ "ecdh_public_jwk": dev.ecdh_jwk, "platform": { "tier": "B" } });
    let first = h
        .send(register_request(
            &dev.proof("POST", "/kernel/v1/devices", None, None, &Tweak::default()),
            &body,
        ))
        .await;
    let nonce = first
        .headers
        .get("dpop-nonce")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let ok = h
        .send(register_request(
            &dev.proof(
                "POST",
                "/kernel/v1/devices",
                Some(&nonce),
                None,
                &Tweak::default(),
            ),
            &body,
        ))
        .await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.body);
    let device_id = Uuid::parse_str(ok.body["device_id"].as_str().unwrap()).unwrap();
    h.kernel
        .devices
        .attach_user(device_id, Uuid::now_v7(), 6)
        .await
        .unwrap();
    ok.body["access_token"].as_str().unwrap().to_owned()
}

/// POST `body` to a DPoP-protected route as `dev` holding `token`.
pub async fn post_json(
    h: &Harness,
    dev: &TestDevice,
    token: &str,
    path: &str,
    body: &Value,
) -> Response {
    let proof = dev.proof(
        "POST",
        path,
        Some(&h.nonce()),
        Some(token),
        &Tweak::default(),
    );
    h.send(
        Request::post(path)
            .header("authorization", format!("DPoP {token}"))
            .header("dpop", proof)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}
