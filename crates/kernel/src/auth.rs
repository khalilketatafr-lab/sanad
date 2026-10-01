//! DPoP enforcement shared by every Kernel route.
//!
//! [`check_proof`] runs the full RFC 9449 pipeline: stateless verification,
//! then nonce, then `jti` replay. The nonce comes before replay so that
//! proofs rejected for a missing nonce never consume cache space.
//! [`DpopAuth`] adds token binding for protected routes:
//!
//! ```text
//! proof.jkt == token.cnf.jkt == devices[token.sub].dpop_jkt   and the device is not revoked
//! ```

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, header};

use crate::devices::Device;
use crate::dpop::{Request, VerifiedProof};
use crate::error::{ApiError, Role};
use crate::replay::Seen;
use crate::token::AccessClaims;
use crate::{AppState, Kernel, unix_now};

/// Exactly one header value, as a string.
fn single<'h>(headers: &'h HeaderMap, name: &str) -> Result<Option<&'h str>, ()> {
    let mut it = headers.get_all(name).iter();
    match (it.next(), it.next()) {
        (None, _) => Ok(None),
        (Some(v), None) => v.to_str().map(Some).map_err(|_| ()),
        (Some(_), Some(_)) => Err(()),
    }
}

pub fn check_proof(
    k: &Kernel,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
    access_token: Option<&str>,
    role: Role,
) -> Result<VerifiedProof, ApiError> {
    let now = unix_now();
    let fresh = || k.nonces.issue(now);
    let invalid = |detail: String| ApiError::InvalidDpopProof {
        role,
        detail,
        nonce: fresh(),
    };

    let proof = match single(headers, "dpop") {
        Ok(Some(p)) => p,
        Ok(None) => return Err(invalid("DPoP proof required".into())),
        Err(()) => return Err(invalid("exactly one DPoP header is allowed".into())),
    };
    let url = k.url_for(path).ok_or(ApiError::Internal)?;
    let req = Request {
        method: method.as_str(),
        url: &url,
        now,
        access_token,
    };
    let verified = k
        .verifier
        .verify(proof, &req)
        .map_err(|e| invalid(e.to_string()))?;

    if !verified
        .nonce
        .as_deref()
        .is_some_and(|n| k.nonces.is_valid(n, now))
    {
        return Err(ApiError::UseDpopNonce {
            role,
            nonce: fresh(),
        });
    }
    match k.replay.check_and_insert(&verified.jkt, &verified.jti, now) {
        Seen::First => Ok(verified),
        Seen::Replay => Err(invalid("DPoP proof jti was already used".into())),
        Seen::Overloaded => Err(ApiError::Unavailable("replay cache saturated")),
    }
}

/// Authenticated device on a protected route.
#[derive(Debug)]
pub struct DpopAuth {
    pub device: Device,
    pub claims: AccessClaims,
    pub proof: VerifiedProof,
}

impl FromRequestParts<AppState> for DpopAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, k: &AppState) -> Result<Self, ApiError> {
        let now = unix_now();
        let token_error = |detail: &'static str| ApiError::InvalidToken {
            detail,
            nonce: Some(k.nonces.issue(now)),
        };

        let authz = single(&parts.headers, header::AUTHORIZATION.as_str())
            .map_err(|()| token_error("exactly one Authorization header is allowed"))?
            .ok_or_else(|| token_error("DPoP access token required"))?;
        let (scheme, token) = authz
            .split_once(' ')
            .ok_or_else(|| token_error("malformed Authorization header"))?;
        if scheme.eq_ignore_ascii_case("bearer") {
            // RFC 9449 §7.2: a DPoP-bound token must not be accepted as a bearer token.
            return Err(token_error(
                "DPoP-bound token presented with the Bearer scheme",
            ));
        }
        if !scheme.eq_ignore_ascii_case("dpop") {
            return Err(token_error("unsupported authorization scheme"));
        }
        let token = token.trim();
        let claims = k
            .tokens
            .verify(token)
            .map_err(|_| token_error("access token invalid or expired"))?;
        let proof = check_proof(
            k,
            &parts.headers,
            &parts.method,
            parts.uri.path(),
            Some(token),
            Role::ResourceServer,
        )?;

        if proof.jkt != claims.jkt {
            return Err(token_error("access token is bound to a different key"));
        }
        let device = k
            .devices
            .get(claims.device_id)
            .await
            .map_err(|_| ApiError::Internal)?
            .ok_or_else(|| token_error("unknown device"))?;
        if device.revoked_at.is_some() {
            return Err(token_error("device revoked"));
        }
        if device.dpop_jkt != proof.jkt {
            return Err(token_error("key is not registered to this device"));
        }
        Ok(Self {
            device,
            claims,
            proof,
        })
    }
}
