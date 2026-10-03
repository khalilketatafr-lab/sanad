//! `POST /kernel/v1/editions/{id}:open`: the first lease of a reading session.
//!
//! Phase 0 stub of blueprint 01 §2: the entitlement is "free editions open
//! for any registered device", the variant vector is all-A, and the window is
//! a fixed number of chunks from the requested start. What is real: the chunk
//! keys are derived from the edition's TMK, wrapped to *this* device's ECDH
//! key under a fresh ephemeral key (see [`crate::lease`]), and the lease
//! carries a Reading Capability Token bound to the device's DPoP key.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::DpopAuth;
use crate::error::{ApiError, DPOP_NONCE};
use crate::jose::{P256PublicKey, b64url};
use crate::lease::wrap_for_device;
use crate::policy::{Entitlement, OpenContext, Principal, SAMPLE_CHAPTERS};
use crate::token::RctClaims;
use crate::{AppState, unix_now};

pub const PATH: &str = "/kernel/v1/editions/{op}";
/// Chunks per stub lease window.
pub const WINDOW: u32 = 3;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    /// First chunk of the window (default 0: the beginning, or a sync anchor).
    #[serde(default)]
    pub start: u32,
}

#[derive(Debug, Serialize)]
pub struct LeaseKey {
    pub chunk: u32,
    pub variant: u8,
    /// AES-KW (RFC 3394) of the 256-bit chunk key, base64url.
    pub wrapped: String,
}

#[derive(Debug, Serialize)]
pub struct LeaseResponse {
    pub lease_id: Uuid,
    pub edition_id: Uuid,
    /// `[start, end)` chunk window.
    pub window: [u32; 2],
    pub expires_in: u64,
    /// The lease's ephemeral ECDH public key (P-256 JWK).
    pub ephemeral_public_jwk: Value,
    pub keys: Vec<LeaseKey>,
    /// Reading Capability Token (PASETO v4.public).
    pub rct: String,
}

pub async fn open(
    State(k): State<AppState>,
    Path(op): Path<String>,
    auth: DpopAuth,
    body: Option<Json<OpenRequest>>,
) -> Result<(HeaderMap, Json<LeaseResponse>), ApiError> {
    let id = op
        .strip_suffix(":open")
        .ok_or(ApiError::NotFound("unknown edition operation"))?;
    let edition_id = Uuid::parse_str(id)
        .map_err(|_| ApiError::BadRequest("edition id must be a UUID".into()))?;
    let edition = k
        .catalog
        .get(&edition_id)
        .ok_or(ApiError::NotFound("unknown edition"))?;
    let start = body.map(|Json(b)| b.start).unwrap_or_default();
    if start >= edition.chunks {
        return Err(ApiError::BadRequest(
            "window start is past the last chunk".into(),
        ));
    }
    // Entitlement authorization (blueprint 01 §2). The principal and the
    // entitlement kind come from Kernel-owned data, never the request. Session
    // and device counts are prospective; the atomic session store (B.2) will
    // supply real counts — until then an open counts as one of each.
    let principal = match auth.device.user_id {
        Some(uid) => Principal::User(uid.to_string()),
        None => Principal::Anonymous,
    };
    let sample = matches!(&principal, Principal::Anonymous) && edition.free;
    let entitlement = match (edition.free, &principal) {
        (true, Principal::User(_)) => Entitlement::Free,
        (true, Principal::Anonymous) => Entitlement::Sample,
        // Phase 1a has no purchase/subscription store: a non-free edition has
        // no entitlement yet, which the policy denies.
        (false, _) => Entitlement::Purchase,
    };
    k.policy
        .evaluate_open(
            &principal,
            &edition_id.to_string(),
            &OpenContext {
                entitlement,
                active_sessions: 1,
                active_devices: 1,
                chapter: start,
            },
        )
        .map_err(|_| ApiError::Forbidden("not authorized to open this edition"))?;
    // Anonymous sampling never reaches past the sample boundary, even
    // though the window would otherwise extend WINDOW chunks ahead.
    let sample_ceiling = if sample { SAMPLE_CHAPTERS } else { u32::MAX };
    let end = (start + WINDOW).min(edition.chunks).min(sample_ceiling);

    let variant = 0u8; // Phase 0: all-A variant vector.
    let keys = (start..end)
        .map(|i| edition.chunk_key(i, variant).map(|ck| (i, ck)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::Internal)?;
    let refs: Vec<_> = keys.iter().map(|(i, ck)| (*i, variant, ck)).collect();
    let lease_id = Uuid::now_v7();
    let lease = wrap_for_device(&auth.device.ecdh_public, &lease_id, &refs).map_err(|e| {
        tracing::error!(device = %auth.device.id, error = %e, "lease wrap failed");
        ApiError::Internal
    })?;
    let ephemeral = P256PublicKey::from_uncompressed(&lease.ephemeral_public)
        .map_err(|_| ApiError::Internal)?;

    let ttl = k.config.token_ttl;
    let rct = k
        .tokens
        .issue_rct(
            &RctClaims {
                device_id: auth.device.id,
                jkt: auth.proof.jkt.clone(),
                edition: edition_id,
                lease_id,
                window: [start, end],
                profile: "standard".into(),
            },
            ttl,
        )
        .map_err(|_| ApiError::Internal)?;
    tracing::info!(device = %auth.device.id, edition = %edition_id, lease = %lease_id, start, end, "lease issued");

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&k.nonces.issue(unix_now())) {
        headers.insert(DPOP_NONCE, v);
    }
    Ok((
        headers,
        Json(LeaseResponse {
            lease_id,
            edition_id,
            window: [start, end],
            expires_in: ttl.as_secs(),
            ephemeral_public_jwk: ephemeral.to_jwk(),
            keys: lease
                .keys
                .iter()
                .map(|w| LeaseKey {
                    chunk: w.chunk,
                    variant: w.variant,
                    wrapped: b64url(&w.wrapped),
                })
                .collect(),
            rct,
        }),
    ))
}
