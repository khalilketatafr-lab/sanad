//! RFC 9449 DPoP proof verification (§4.3), on aws-lc-rs.
//!
//! Deliberately narrow: ES256 only. Device keys are P-256 WebCrypto keys, so
//! accepting RSA or other algorithms would only widen the attack surface.
//!
//! Checked here (stateless): size, `typ`, `alg`, `jwk` (public, P-256), the
//! signature, `htm`, `htu` (normalized, no query or fragment), `iat` window,
//! `jti` shape and, when an access token is presented, `ath`.
//! Checked by the caller (stateful): `nonce` ([`crate::nonce`]), `jti`
//! replay ([`crate::replay`]) and key binding to the token's `cnf.jkt` and
//! the registered device ([`crate::auth`]).

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::jose::{JoseError, P256PublicKey, access_token_hash, b64url_decode};

pub const DPOP_TYP: &str = "dpop+jwt";
pub const DPOP_ALG: &str = "ES256";
const MAX_JTI_LEN: usize = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DpopError {
    #[error("DPoP proof too large")]
    TooLarge,
    #[error("DPoP proof is not a compact JWS")]
    Malformed,
    #[error("DPoP proof typ must be dpop+jwt")]
    BadType,
    #[error("DPoP proof alg must be ES256")]
    BadAlgorithm,
    #[error("DPoP proof jwk: {0}")]
    BadKey(JoseError),
    #[error("DPoP proof signature invalid")]
    BadSignature,
    #[error("DPoP proof htm does not match the request method")]
    MethodMismatch,
    #[error("DPoP proof htu does not match the request URL")]
    UrlMismatch,
    #[error("DPoP proof iat outside the acceptable window")]
    Stale,
    #[error("DPoP proof jti missing or invalid")]
    BadJti,
    #[error("DPoP proof ath missing or does not match the access token")]
    AthMismatch,
}

#[derive(Deserialize)]
struct Header {
    typ: Option<String>,
    alg: Option<String>,
    jwk: Option<Value>,
}

#[derive(Deserialize)]
struct Claims {
    jti: Option<String>,
    htm: Option<String>,
    htu: Option<String>,
    iat: Option<i64>,
    nonce: Option<String>,
    ath: Option<String>,
}

/// What the request presents to the verifier.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// The request's canonical external URL (configured public origin + path).
    pub url: &'a Url,
    /// Seconds since the Unix epoch.
    pub now: i64,
    /// Present on resource requests (`Authorization: DPoP <token>`); requires `ath`.
    pub access_token: Option<&'a str>,
}

/// A proof that passed every stateless check.
#[derive(Debug, Clone)]
pub struct VerifiedProof {
    pub key: P256PublicKey,
    /// RFC 7638 thumbprint of the proof key.
    pub jkt: String,
    pub jti: String,
    pub iat: i64,
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct DpopVerifier {
    /// Accept |now − iat| up to this many seconds (freshness is mainly from the nonce).
    pub max_skew_secs: i64,
    pub max_proof_bytes: usize,
}

impl Default for DpopVerifier {
    fn default() -> Self {
        Self {
            max_skew_secs: 60,
            max_proof_bytes: 4096,
        }
    }
}

/// `htu` comparison per RFC 9449 §4.3: ignore query and fragment; compare the
/// rest after URL normalization (scheme/host case, default ports, dot segments).
fn htu_matches(claimed: &str, expected: &Url) -> bool {
    let Ok(mut claimed) = Url::parse(claimed) else {
        return false;
    };
    claimed.set_query(None);
    claimed.set_fragment(None);
    let mut expected = expected.clone();
    expected.set_query(None);
    expected.set_fragment(None);
    claimed == expected
}

impl DpopVerifier {
    pub fn verify(&self, proof: &str, req: &Request<'_>) -> Result<VerifiedProof, DpopError> {
        if proof.len() > self.max_proof_bytes {
            return Err(DpopError::TooLarge);
        }
        let mut parts = proof.split('.');
        let (Some(h64), Some(p64), Some(s64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(DpopError::Malformed);
        };

        let header: Header = b64url_decode(h64)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(DpopError::Malformed)?;
        if header.typ.as_deref() != Some(DPOP_TYP) {
            return Err(DpopError::BadType);
        }
        if header.alg.as_deref() != Some(DPOP_ALG) {
            return Err(DpopError::BadAlgorithm);
        }
        let key = P256PublicKey::from_jwk(
            header
                .jwk
                .as_ref()
                .ok_or(DpopError::BadKey(JoseError::NotP256))?,
        )
        .map_err(DpopError::BadKey)?;

        // Signature over the exact received ASCII: "header.payload".
        let signature = b64url_decode(s64).map_err(|_| DpopError::Malformed)?;
        let signing_input_len = h64.len() + 1 + p64.len();
        key.verify_es256(&proof.as_bytes()[..signing_input_len], &signature)
            .map_err(|_| DpopError::BadSignature)?;

        let claims: Claims = b64url_decode(p64)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(DpopError::Malformed)?;
        if claims.htm.as_deref() != Some(req.method) {
            return Err(DpopError::MethodMismatch);
        }
        if !claims
            .htu
            .as_deref()
            .is_some_and(|u| htu_matches(u, req.url))
        {
            return Err(DpopError::UrlMismatch);
        }
        let iat = claims.iat.ok_or(DpopError::Stale)?;
        if (req.now - iat).abs() > self.max_skew_secs {
            return Err(DpopError::Stale);
        }
        let jti = claims
            .jti
            .filter(|j| !j.is_empty() && j.len() <= MAX_JTI_LEN && j.is_ascii())
            .ok_or(DpopError::BadJti)?;
        if let Some(token) = req.access_token
            && claims.ath.as_deref() != Some(access_token_hash(token).as_str())
        {
            return Err(DpopError::AthMismatch);
        }
        let jkt = key.thumbprint();
        Ok(VerifiedProof {
            key,
            jkt,
            jti,
            iat,
            nonce: claims.nonce,
        })
    }
}
