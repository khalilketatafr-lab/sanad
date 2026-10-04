//! DPoP-bound access tokens: PASETO v4.public (Ed25519).
//!
//! Claims: `iss`, `aud`, `sub` (device id), `jti`, `iat`/`nbf`/`exp`,
//! `cnf.jkt` (RFC 9449 §6.1: the thumbprint of the key that must sign every
//! proof presented with this token) and `tier`.
//!
//! Tokens are domain-separated by PASETO's implicit assertion: the bytes
//! `sanad-kernel/access/v1` are signed but never transmitted, so a v4.public
//! token minted for another purpose by the same key can never verify as an
//! access token, and vice versa.
//!
//! The same issuer mints **Reading Capability Tokens** (blueprint 01 §3.4,
//! implicit assertion `sanad-kernel/rct/v1`). Each lease carries one, and
//! Oracle, quotes and accessibility verify it without a database round trip.

use core::time::Duration;

use pasetors::claims::{Claims, ClaimsValidationRules};
use pasetors::keys::{AsymmetricKeyPair, AsymmetricPublicKey, AsymmetricSecretKey, Generate};
use pasetors::token::UntrustedToken;
use pasetors::version4::V4;
use pasetors::{Public, public};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

pub const ISSUER: &str = "sanad-kernel";
pub const AUDIENCE: &str = "sanad";
const IMPLICIT: &[u8] = b"sanad-kernel/access/v1";
const IMPLICIT_RCT: &[u8] = b"sanad-kernel/rct/v1";
const IMPLICIT_CEREMONY: &[u8] = b"sanad-kernel/webauthn/v1";
/// How long a passkey ceremony may take (WebAuthn timeout + margin).
pub const CEREMONY_TTL: Duration = Duration::from_mins(5);

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("token key: {0}")]
    Key(String),
    #[error("token could not be issued")]
    Issue,
    #[error("access token invalid or expired")]
    Invalid,
    #[error("policy engine: {0}")]
    Policy(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessClaims {
    pub device_id: Uuid,
    /// cnf.jkt: the only key allowed to sign proofs for this token.
    pub jkt: String,
    pub tier: String,
    /// The passkey account the device is signed in to (`uid`), if any.
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeremonyMode {
    Register,
    Authenticate,
}

/// State of a WebAuthn ceremony between `passkey:begin` and `passkey:finish`,
/// carried by the client as a signed token (no server-side session). It is
/// bound to the device's DPoP key and used once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeremonyClaims {
    pub mode: CeremonyMode,
    pub challenge: [u8; 32],
    pub jkt: String,
    /// Registration: the account the new passkey belongs to.
    pub user_id: Option<Uuid>,
    /// Registration: the WebAuthn user handle sent to the authenticator.
    pub user_handle: Option<Vec<u8>>,
}

/// What a lease grants, for services that never see the lease itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RctClaims {
    pub device_id: Uuid,
    /// The DPoP key every request presenting this RCT must be signed with.
    pub jkt: String,
    pub edition: Uuid,
    pub lease_id: Uuid,
    /// Chunk window `[start, end)` the lease covers.
    pub window: [u32; 2],
    /// Protection profile (`standard` or `vault`).
    pub profile: String,
    /// 64-bit reading-session id; the Ex Libris watermark seed (05 §4).
    pub session_id: u64,
}

pub struct TokenIssuer {
    secret: AsymmetricSecretKey<V4>,
    public: AsymmetricPublicKey<V4>,
    ttl: Duration,
}

impl core::fmt::Debug for TokenIssuer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TokenIssuer")
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl TokenIssuer {
    /// From a 64-byte Ed25519 secret key (seed ‖ public key).
    pub fn from_secret_bytes(bytes: &[u8], ttl: Duration) -> Result<Self, TokenError> {
        let secret =
            AsymmetricSecretKey::<V4>::from(bytes).map_err(|e| TokenError::Key(e.to_string()))?;
        let public = AsymmetricPublicKey::<V4>::try_from(&secret)
            .map_err(|e| TokenError::Key(e.to_string()))?;
        Ok(Self {
            secret,
            public,
            ttl,
        })
    }

    /// Fresh random key: development and tests only.
    pub fn generate(ttl: Duration) -> Result<Self, TokenError> {
        let kp = AsymmetricKeyPair::<V4>::generate().map_err(|e| TokenError::Key(e.to_string()))?;
        Ok(Self {
            secret: kp.secret,
            public: kp.public,
            ttl,
        })
    }

    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn issue(&self, claims: &AccessClaims) -> Result<String, TokenError> {
        let mut c = Claims::new_expires_in(&self.ttl).map_err(|_| TokenError::Issue)?;
        c.issuer(ISSUER).map_err(|_| TokenError::Issue)?;
        c.audience(AUDIENCE).map_err(|_| TokenError::Issue)?;
        c.subject(&claims.device_id.to_string())
            .map_err(|_| TokenError::Issue)?;
        c.token_identifier(&Uuid::new_v4().to_string())
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("cnf", serde_json::json!({ "jkt": claims.jkt }))
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("tier", claims.tier.clone())
            .map_err(|_| TokenError::Issue)?;
        if let Some(uid) = claims.user_id {
            c.add_additional("uid", uid.to_string())
                .map_err(|_| TokenError::Issue)?;
        }
        public::sign(&self.secret, &c, None, Some(IMPLICIT)).map_err(|_| TokenError::Issue)
    }

    pub fn issue_ceremony(&self, claims: &CeremonyClaims) -> Result<String, TokenError> {
        let mut c = Claims::new_expires_in(&CEREMONY_TTL).map_err(|_| TokenError::Issue)?;
        c.issuer(ISSUER).map_err(|_| TokenError::Issue)?;
        c.audience(AUDIENCE).map_err(|_| TokenError::Issue)?;
        let mode = match claims.mode {
            CeremonyMode::Register => "register",
            CeremonyMode::Authenticate => "authenticate",
        };
        c.add_additional("mode", mode)
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("chl", crate::jose::b64url(&claims.challenge))
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("jkt", claims.jkt.clone())
            .map_err(|_| TokenError::Issue)?;
        if let Some(uid) = claims.user_id {
            c.add_additional("uid", uid.to_string())
                .map_err(|_| TokenError::Issue)?;
        }
        if let Some(h) = &claims.user_handle {
            c.add_additional("uh", crate::jose::b64url(h))
                .map_err(|_| TokenError::Issue)?;
        }
        public::sign(&self.secret, &c, None, Some(IMPLICIT_CEREMONY)).map_err(|_| TokenError::Issue)
    }

    pub fn verify_ceremony(&self, token: &str) -> Result<CeremonyClaims, TokenError> {
        let mut rules = ClaimsValidationRules::new();
        rules.validate_issuer_with(ISSUER);
        rules.validate_audience_with(AUDIENCE);
        let untrusted =
            UntrustedToken::<Public, V4>::try_from(token).map_err(|_| TokenError::Invalid)?;
        let trusted = public::verify(
            &self.public,
            &untrusted,
            &rules,
            None,
            Some(IMPLICIT_CEREMONY),
        )
        .map_err(|_| TokenError::Invalid)?;
        let c = trusted.payload_claims().ok_or(TokenError::Invalid)?;
        let text = |k: &str| c.get_claim(k).and_then(Value::as_str);
        let mode = match text("mode") {
            Some("register") => CeremonyMode::Register,
            Some("authenticate") => CeremonyMode::Authenticate,
            _ => return Err(TokenError::Invalid),
        };
        let challenge: [u8; 32] = text("chl")
            .and_then(|s| crate::jose::b64url_decode(s).ok())
            .and_then(|b| b.try_into().ok())
            .ok_or(TokenError::Invalid)?;
        Ok(CeremonyClaims {
            mode,
            challenge,
            jkt: text("jkt").ok_or(TokenError::Invalid)?.to_owned(),
            user_id: text("uid")
                .map(Uuid::parse_str)
                .transpose()
                .map_err(|_| TokenError::Invalid)?,
            user_handle: text("uh")
                .map(crate::jose::b64url_decode)
                .transpose()
                .map_err(|_| TokenError::Invalid)?,
        })
    }

    /// A Reading Capability Token, valid for the lease TTL.
    pub fn issue_rct(
        &self,
        claims: &RctClaims,
        ttl: core::time::Duration,
    ) -> Result<String, TokenError> {
        let mut c = Claims::new_expires_in(&ttl).map_err(|_| TokenError::Issue)?;
        c.issuer(ISSUER).map_err(|_| TokenError::Issue)?;
        c.audience(AUDIENCE).map_err(|_| TokenError::Issue)?;
        c.subject(&claims.device_id.to_string())
            .map_err(|_| TokenError::Issue)?;
        c.token_identifier(&claims.lease_id.to_string())
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("dev", format!("jkt:{}", claims.jkt))
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("ed", claims.edition.to_string())
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("win", serde_json::json!(claims.window))
            .map_err(|_| TokenError::Issue)?;
        c.add_additional("pro", claims.profile.clone())
            .map_err(|_| TokenError::Issue)?;
        // u64 can exceed JSON's safe-integer range, so carry it as a string.
        c.add_additional("sid", claims.session_id.to_string())
            .map_err(|_| TokenError::Issue)?;
        public::sign(&self.secret, &c, None, Some(IMPLICIT_RCT)).map_err(|_| TokenError::Issue)
    }

    /// Verifies an RCT (signature, purpose, iss/aud, expiry) and returns its claims.
    pub fn verify_rct(&self, token: &str) -> Result<RctClaims, TokenError> {
        let mut rules = ClaimsValidationRules::new();
        rules.validate_issuer_with(ISSUER);
        rules.validate_audience_with(AUDIENCE);
        let untrusted =
            UntrustedToken::<Public, V4>::try_from(token).map_err(|_| TokenError::Invalid)?;
        let trusted = public::verify(&self.public, &untrusted, &rules, None, Some(IMPLICIT_RCT))
            .map_err(|_| TokenError::Invalid)?;
        let c = trusted.payload_claims().ok_or(TokenError::Invalid)?;
        let str_claim = |k: &str| {
            c.get_claim(k)
                .and_then(Value::as_str)
                .ok_or(TokenError::Invalid)
        };
        let uuid = |k: &str| {
            str_claim(k).and_then(|s| Uuid::parse_str(s).map_err(|_| TokenError::Invalid))
        };
        let window = c
            .get_claim("win")
            .and_then(Value::as_array)
            .and_then(|w| match w.as_slice() {
                [a, b] => Some([
                    u32::try_from(a.as_u64()?).ok()?,
                    u32::try_from(b.as_u64()?).ok()?,
                ]),
                _ => None,
            })
            .ok_or(TokenError::Invalid)?;
        Ok(RctClaims {
            device_id: uuid("sub")?,
            jkt: str_claim("dev")?
                .strip_prefix("jkt:")
                .ok_or(TokenError::Invalid)?
                .to_owned(),
            edition: uuid("ed")?,
            lease_id: uuid("jti")?,
            window,
            profile: str_claim("pro")?.to_owned(),
            session_id: str_claim("sid")?.parse().map_err(|_| TokenError::Invalid)?,
        })
    }

    /// Verifies signature, implicit assertion, iss/aud and iat/nbf/exp.
    pub fn verify(&self, token: &str) -> Result<AccessClaims, TokenError> {
        let mut rules = ClaimsValidationRules::new();
        rules.validate_issuer_with(ISSUER);
        rules.validate_audience_with(AUDIENCE);
        let untrusted =
            UntrustedToken::<Public, V4>::try_from(token).map_err(|_| TokenError::Invalid)?;
        let trusted = public::verify(&self.public, &untrusted, &rules, None, Some(IMPLICIT))
            .map_err(|_| TokenError::Invalid)?;
        let claims = trusted.payload_claims().ok_or(TokenError::Invalid)?;
        let device_id = claims
            .get_claim("sub")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(TokenError::Invalid)?;
        let jkt = claims
            .get_claim("cnf")
            .and_then(|c| c.get("jkt"))
            .and_then(Value::as_str)
            .ok_or(TokenError::Invalid)?
            .to_owned();
        let tier = claims
            .get_claim("tier")
            .and_then(Value::as_str)
            .unwrap_or("anonymous")
            .to_owned();
        let user_id = claims
            .get_claim("uid")
            .and_then(Value::as_str)
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| TokenError::Invalid)?;
        Ok(AccessClaims {
            device_id,
            jkt,
            tier,
            user_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> AccessClaims {
        AccessClaims {
            device_id: Uuid::now_v7(),
            jkt: "abc".into(),
            tier: "anonymous".into(),
            user_id: None,
        }
    }

    #[test]
    fn round_trip_and_tamper() {
        let t = TokenIssuer::generate(Duration::from_mins(15)).unwrap_or_else(|e| panic!("{e}"));
        let token = t.issue(&claims()).unwrap_or_else(|e| panic!("{e}"));
        assert!(token.starts_with("v4.public."));
        assert_eq!(t.verify(&token).map(|c| c.jkt).ok().as_deref(), Some("abc"));
        let mut bad = token.clone().into_bytes();
        let i = bad.len() - 10;
        bad[i] = if bad[i] == b'A' { b'B' } else { b'A' };
        assert!(t.verify(&String::from_utf8_lossy(&bad)).is_err());
    }

    #[test]
    fn other_keys_and_other_purposes_are_rejected() {
        let a = TokenIssuer::generate(Duration::from_mins(15)).unwrap_or_else(|e| panic!("{e}"));
        let b = TokenIssuer::generate(Duration::from_mins(15)).unwrap_or_else(|e| panic!("{e}"));
        let token = a.issue(&claims()).unwrap_or_else(|e| panic!("{e}"));
        assert!(b.verify(&token).is_err(), "wrong key");
        // Same key, different implicit assertion: a token for another purpose.
        let c = Claims::new().unwrap_or_else(|e| panic!("{e}"));
        let other = public::sign(&a.secret, &c, None, Some(b"sanad-kernel/rct/v1"))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(a.verify(&other).is_err(), "cross-purpose token accepted");
    }

    #[test]
    fn rct_round_trip_and_purpose_separation() {
        let t = TokenIssuer::generate(Duration::from_mins(15)).unwrap_or_else(|e| panic!("{e}"));
        let rct = RctClaims {
            device_id: Uuid::now_v7(),
            jkt: "abc".into(),
            edition: Uuid::now_v7(),
            lease_id: Uuid::now_v7(),
            window: [20, 25],
            profile: "standard".into(),
            session_id: 0xDEAD_BEEF_0000_0001,
        };
        let token = t
            .issue_rct(&rct, Duration::from_mins(15))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.verify_rct(&token).ok(), Some(rct.clone()));
        assert!(t.verify(&token).is_err(), "an RCT is not an access token");
        let access = t.issue(&claims()).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            t.verify_rct(&access).is_err(),
            "an access token is not an RCT"
        );
    }
}
