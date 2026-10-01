//! Minimal, strict JOSE for P-256 device keys: base64url, EC JWK parsing,
//! RFC 7638 thumbprints and ES256 verification. Only what the Kernel needs,
//! all on aws-lc-rs.

use aws_lc_rs::agreement::{
    self, ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey as AgreementPublicKey,
};
use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JoseError {
    #[error("invalid base64url")]
    Base64,
    #[error("JWK must be an EC P-256 public key")]
    NotP256,
    #[error("JWK contains private key material")]
    PrivateMaterial,
    #[error("JWK coordinate has wrong length")]
    CoordinateLength,
    #[error("public key is not a valid P-256 point")]
    InvalidPoint,
    #[error("signature verification failed")]
    BadSignature,
}

/// Unpadded base64url (RFC 4648 §5), as used throughout JOSE.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Strict unpadded base64url decoding: rejects padding and non-canonical encodings.
pub fn b64url_decode(s: &str) -> Result<Vec<u8>, JoseError> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| JoseError::Base64)
}

/// A P-256 public key (affine coordinates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P256PublicKey {
    x: [u8; 32],
    y: [u8; 32],
}

fn coordinate(v: Option<&Value>) -> Result<[u8; 32], JoseError> {
    let s = v.and_then(Value::as_str).ok_or(JoseError::NotP256)?;
    let bytes = b64url_decode(s)?;
    bytes.try_into().map_err(|_| JoseError::CoordinateLength)
}

impl P256PublicKey {
    /// Parses a public EC JWK. Extra members (`ext`, `key_ops`, … as emitted
    /// by WebCrypto's `exportKey("jwk")`) are allowed; private material is not.
    pub fn from_jwk(jwk: &Value) -> Result<Self, JoseError> {
        let obj = jwk.as_object().ok_or(JoseError::NotP256)?;
        if obj.get("kty").and_then(Value::as_str) != Some("EC")
            || obj.get("crv").and_then(Value::as_str) != Some("P-256")
        {
            return Err(JoseError::NotP256);
        }
        if obj.contains_key("d") {
            return Err(JoseError::PrivateMaterial);
        }
        Ok(Self {
            x: coordinate(obj.get("x"))?,
            y: coordinate(obj.get("y"))?,
        })
    }

    /// SEC1 uncompressed point: 0x04 ‖ X ‖ Y.
    #[must_use]
    pub fn uncompressed(&self) -> [u8; 65] {
        let mut out = [0u8; 65];
        out[0] = 0x04;
        out[1..33].copy_from_slice(&self.x);
        out[33..].copy_from_slice(&self.y);
        out
    }

    /// Inverse of [`Self::uncompressed`].
    pub fn from_uncompressed(bytes: &[u8]) -> Result<Self, JoseError> {
        if bytes.len() != 65 || bytes[0] != 0x04 {
            return Err(JoseError::InvalidPoint);
        }
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        x.copy_from_slice(&bytes[1..33]);
        y.copy_from_slice(&bytes[33..]);
        Ok(Self { x, y })
    }

    /// The public JWK with only the required members, in canonical form.
    #[must_use]
    pub fn to_jwk(&self) -> Value {
        serde_json::json!({ "kty": "EC", "crv": "P-256", "x": b64url(&self.x), "y": b64url(&self.y) })
    }

    /// RFC 7638 JWK thumbprint (SHA-256, base64url): the required members in
    /// lexicographic order, no whitespace. This is the DPoP `jkt`.
    #[must_use]
    pub fn thumbprint(&self) -> String {
        let canonical = format!(
            r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
            b64url(&self.x),
            b64url(&self.y)
        );
        b64url(digest(&SHA256, canonical.as_bytes()).as_ref())
    }

    /// ES256 (ECDSA P-256 / SHA-256) over `message`, with the JWS fixed-width
    /// `r ‖ s` signature encoding (what WebCrypto produces).
    pub fn verify_es256(&self, message: &[u8], signature: &[u8]) -> Result<(), JoseError> {
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, self.uncompressed())
            .verify(message, signature)
            .map_err(|_| JoseError::BadSignature)
    }

    /// Rejects points not on the curve before we ever wrap keys to them
    /// (invalid-curve attacks on ECDH), by performing a throwaway agreement.
    pub fn validate_for_ecdh(&self) -> Result<(), JoseError> {
        let rng = SystemRandom::new();
        let eph =
            EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| JoseError::InvalidPoint)?;
        let peer = self.uncompressed();
        agreement::agree_ephemeral(
            eph,
            AgreementPublicKey::new(&ECDH_P256, peer),
            JoseError::InvalidPoint,
            |_| Ok(()),
        )
    }
}

/// `base64url(SHA-256(ASCII(token)))`: the DPoP `ath` claim (RFC 9449 §4.2).
#[must_use]
pub fn access_token_hash(token: &str) -> String {
    b64url(digest(&SHA256, token.as_bytes()).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7638 §3.1 uses an RSA key; this vector is for P-256 and was
    /// computed independently (Python: hashlib.sha256 over the canonical JSON).
    #[test]
    fn thumbprint_is_rfc7638_canonical() {
        let jwk = serde_json::json!({
            "kty": "EC", "crv": "P-256", "ext": true, "key_ops": [],
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        });
        let key = P256PublicKey::from_jwk(&jwk).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            key.thumbprint(),
            include_str!("../tests/vectors/p256_jkt.txt").trim()
        );
        assert!(key.validate_for_ecdh().is_ok());
    }

    /// RFC 7515 Appendix A.3: the ES256 JWS example, verified with our code path.
    #[test]
    fn verifies_rfc7515_es256_example() {
        let jwk = serde_json::json!({"kty":"EC","crv":"P-256","x":"f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU","y":"x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"});
        let key = P256PublicKey::from_jwk(&jwk).unwrap_or_else(|e| panic!("{e}"));
        let signing_input = "eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let sig = b64url_decode("DtEhU3ljbEg8L38VWAfUAqOyKAM6-Xx-F4GawxaepmXFCgfTjDxw5djxLa8ISlSApmWQxfKTUJqPP3-Kg6NU1Q").unwrap_or_default();
        assert_eq!(key.verify_es256(signing_input.as_bytes(), &sig), Ok(()));
        assert_eq!(
            key.verify_es256(b"tampered", &sig),
            Err(JoseError::BadSignature)
        );
    }

    #[test]
    fn rejects_private_and_foreign_keys() {
        let mut jwk = serde_json::json!({"kty":"EC","crv":"P-256","x":"f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU","y":"x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0","d":"jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI"});
        assert_eq!(
            P256PublicKey::from_jwk(&jwk),
            Err(JoseError::PrivateMaterial)
        );
        jwk["crv"] = "P-384".into();
        assert_eq!(P256PublicKey::from_jwk(&jwk), Err(JoseError::NotP256));
        let rsa = serde_json::json!({"kty":"RSA","n":"AQAB","e":"AQAB"});
        assert_eq!(P256PublicKey::from_jwk(&rsa), Err(JoseError::NotP256));
    }

    #[test]
    fn rejects_off_curve_points() {
        let bad = P256PublicKey {
            x: [1; 32],
            y: [2; 32],
        };
        assert_eq!(bad.validate_for_ecdh(), Err(JoseError::InvalidPoint));
        assert_eq!(
            bad.verify_es256(b"m", &[0; 64]),
            Err(JoseError::BadSignature)
        );
    }
}
