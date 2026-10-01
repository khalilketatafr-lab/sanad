//! `POST /kernel/v1/devices`: device key registration.
//! `GET  /kernel/v1/devices/self`: the authenticated device (DPoP-protected).
//!
//! Registration proves possession of `DeviceKey-Sign` (the DPoP proof is
//! signed with it and its JWK is in the proof header), records
//! `DeviceKey-ECDH` (validated on-curve), and issues an access token bound to
//! the signing key (`cnf.jkt`). Anonymous sampling devices register without
//! an account; passkey sign-in later attaches a `user_id`.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::{DpopAuth, check_proof};
use crate::devices::{Device, NewDevice, StoreError};
use crate::error::{ApiError, DPOP_NONCE, Role};
use crate::jose::P256PublicKey;
use crate::token::AccessClaims;
use crate::{AppState, unix_now};

pub const PATH: &str = "/kernel/v1/devices";
pub const SELF_PATH: &str = "/kernel/v1/devices/self";
const MAX_PLATFORM_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    /// Public JWK of `DeviceKey-ECDH` (lease keys are wrapped to it).
    pub ecdh_public_jwk: Value,
    /// Coarse, non-identifying client facts (tier, DRM robustness probe result).
    #[serde(default)]
    pub platform: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub device_id: Uuid,
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub dpop_jkt: String,
}

fn nonce_headers(nonce: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(nonce) {
        h.insert(DPOP_NONCE, v);
    }
    h
}

pub async fn register(
    State(k): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, HeaderMap, Json<RegisterResponse>), ApiError> {
    let proof = check_proof(
        &k,
        &headers,
        &Method::POST,
        PATH,
        None,
        Role::AuthorizationServer,
    )?;

    let ecdh = P256PublicKey::from_jwk(&body.ecdh_public_jwk)
        .map_err(|e| ApiError::BadRequest(format!("ecdh_public_jwk: {e}")))?;
    ecdh.validate_for_ecdh()
        .map_err(|e| ApiError::BadRequest(format!("ecdh_public_jwk: {e}")))?;
    if ecdh == proof.key {
        return Err(ApiError::BadRequest(
            "ECDH and signing keys must be distinct".into(),
        ));
    }
    let platform = body
        .platform
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !platform.is_object() || platform.to_string().len() > MAX_PLATFORM_BYTES {
        return Err(ApiError::BadRequest(
            "platform must be a small JSON object".into(),
        ));
    }

    let device = k
        .devices
        .register(NewDevice {
            dpop_jkt: proof.jkt.clone(),
            ecdh_public: ecdh.uncompressed().to_vec(),
            platform,
        })
        .await
        .map_err(|e| match e {
            StoreError::Revoked => ApiError::Forbidden("this device key has been revoked"),
            StoreError::Backend(_) => ApiError::Internal,
        })?;
    let claims = AccessClaims {
        device_id: device.id,
        jkt: proof.jkt.clone(),
        tier: "anonymous".into(),
    };
    let access_token = k.tokens.issue(&claims).map_err(|_| ApiError::Internal)?;
    tracing::info!(device = %device.id, jkt = %proof.jkt, "device registered");

    Ok((
        StatusCode::CREATED,
        nonce_headers(&k.nonces.issue(unix_now())),
        Json(RegisterResponse {
            device_id: device.id,
            access_token,
            token_type: "DPoP",
            expires_in: k.tokens.ttl().as_secs(),
            dpop_jkt: proof.jkt,
        }),
    ))
}

pub async fn get_self(State(k): State<AppState>, auth: DpopAuth) -> (HeaderMap, Json<Device>) {
    (
        nonce_headers(&k.nonces.issue(unix_now())),
        Json(auth.device),
    )
}
