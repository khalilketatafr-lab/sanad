//! WebAuthn relying party for passkeys: registration (W3C WebAuthn L3 §7.1)
//! and authentication (§7.2) verification.
//!
//! Scope, chosen for consumer passkeys:
//! - **Algorithms:** ES256 (-7), EdDSA/Ed25519 (-8), RS256 (-257, Windows
//!   Hello). This is every algorithm platform authenticators and the
//!   passkey providers use.
//! - **Attestation `none` only.** We request `attestation: "none"`, so
//!   clients strip attestation statements. Any other format is refused rather
//!   than half-verified.
//! - **User verification required.** Sign-in is the biometric/PIN tap; a bare
//!   presence test is not an account login.
//!
//! All cryptography is `aws-lc-rs`; CBOR is `ciborium`. `webauthn-rs` was
//! not used: its core depends on OpenSSL, a second crypto stack and a system
//! C library (`docs/spikes/s4-kernel-dpop.md`).

use std::io::Cursor;

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, ED25519, ParsedPublicKey, RSA_PKCS1_2048_8192_SHA256,
    RsaPublicKeyComponents,
};
use ciborium::Value;
use serde::Deserialize;
use thiserror::Error;

use crate::jose::b64url_decode;

/// COSE algorithm identifiers we accept, in preference order.
pub const ES256: i64 = -7;
pub const EDDSA: i64 = -8;
pub const RS256: i64 = -257;
pub const SUPPORTED_ALGS: [i64; 3] = [ES256, EDDSA, RS256];

/// WebAuthn L3: credential ids are at most 1023 bytes.
pub const MAX_CREDENTIAL_ID: usize = 1023;

const FLAG_UP: u8 = 1 << 0;
const FLAG_UV: u8 = 1 << 2;
const FLAG_BE: u8 = 1 << 3;
const FLAG_BS: u8 = 1 << 4;
const FLAG_AT: u8 = 1 << 6;
const FLAG_ED: u8 = 1 << 7;

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum WebAuthnError {
    #[error("clientDataJSON: {0}")]
    ClientData(&'static str),
    #[error("authenticator data: {0}")]
    AuthData(&'static str),
    #[error("attestation: {0}")]
    Attestation(&'static str),
    #[error("credential public key: {0}")]
    PublicKey(&'static str),
    #[error("user presence and verification are required")]
    UserVerification,
    #[error("signature does not verify")]
    Signature,
    #[error("signature counter went backwards: possible cloned authenticator")]
    Counter,
}

/// The relying party: our RP ID and the web origins ceremonies may run on.
#[derive(Debug, Clone)]
pub struct RelyingParty {
    pub id: String,
    pub name: String,
    pub origins: Vec<String>,
}

impl RelyingParty {
    fn id_hash(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(digest(&SHA256, self.id.as_bytes()).as_ref());
        out
    }
}

#[derive(Debug, Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(rename = "crossOrigin", default)]
    cross_origin: bool,
}

/// §7.1 steps 5–12 / §7.2 steps 10–15: parses clientDataJSON, checks type,
/// challenge and origin, and returns SHA-256(clientDataJSON).
fn verify_client_data(
    rp: &RelyingParty,
    json: &[u8],
    kind: &str,
    challenge: &[u8],
) -> Result<[u8; 32], WebAuthnError> {
    let c: ClientData =
        serde_json::from_slice(json).map_err(|_| WebAuthnError::ClientData("not valid JSON"))?;
    if c.kind != kind {
        return Err(WebAuthnError::ClientData("wrong ceremony type"));
    }
    let got =
        b64url_decode(&c.challenge).map_err(|_| WebAuthnError::ClientData("challenge encoding"))?;
    if aws_lc_rs::constant_time::verify_slices_are_equal(&got, challenge).is_err() {
        return Err(WebAuthnError::ClientData("challenge mismatch"));
    }
    if !rp.origins.contains(&c.origin) {
        return Err(WebAuthnError::ClientData("origin not allowed"));
    }
    if c.cross_origin {
        return Err(WebAuthnError::ClientData(
            "cross-origin ceremonies are not allowed",
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(digest(&SHA256, json).as_ref());
    Ok(out)
}

/// A credential public key in COSE form (RFC 9052/9053), validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoseKey {
    /// Uncompressed SEC1 point (65 bytes), validated on the curve.
    Es256(Vec<u8>),
    Ed25519([u8; 32]),
    /// Big-endian modulus (≥ 2048 bits) and exponent.
    Rs256 {
        n: Vec<u8>,
        e: Vec<u8>,
    },
}

fn map_get(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter()
        .find(|(k, _)| k.as_integer().and_then(|i| i64::try_from(i).ok()) == Some(key))
        .map(|(_, v)| v)
}

fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_integer)
        .and_then(|i| i64::try_from(i).ok())
}

fn bytes(v: Option<&Value>) -> Option<&[u8]> {
    v.and_then(Value::as_bytes).map(Vec::as_slice)
}

impl CoseKey {
    /// Parses and validates a COSE_Key.
    pub fn from_cbor(cose: &[u8]) -> Result<Self, WebAuthnError> {
        let v: Value =
            ciborium::from_reader(cose).map_err(|_| WebAuthnError::PublicKey("not CBOR"))?;
        Self::from_value(&v)
    }

    fn from_value(v: &Value) -> Result<Self, WebAuthnError> {
        let m = v.as_map().ok_or(WebAuthnError::PublicKey("not a map"))?;
        let (kty, alg) = (int(map_get(m, 1)), int(map_get(m, 3)));
        match (kty, alg) {
            (Some(2), Some(ES256)) => {
                if int(map_get(m, -1)) != Some(1) {
                    return Err(WebAuthnError::PublicKey("ES256 requires crv P-256"));
                }
                let (x, y) = (bytes(map_get(m, -2)), bytes(map_get(m, -3)));
                let (Some(x), Some(y)) = (x, y) else {
                    return Err(WebAuthnError::PublicKey("missing x or y"));
                };
                if x.len() != 32 || y.len() != 32 {
                    return Err(WebAuthnError::PublicKey("bad coordinate length"));
                }
                let point = [&[0x04][..], x, y].concat();
                ParsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &point)
                    .map_err(|_| WebAuthnError::PublicKey("point is not on P-256"))?;
                Ok(Self::Es256(point))
            }
            (Some(1), Some(EDDSA)) => {
                if int(map_get(m, -1)) != Some(6) {
                    return Err(WebAuthnError::PublicKey("EdDSA requires crv Ed25519"));
                }
                let x: [u8; 32] = bytes(map_get(m, -2))
                    .and_then(|b| b.try_into().ok())
                    .ok_or(WebAuthnError::PublicKey("Ed25519 key must be 32 bytes"))?;
                ParsedPublicKey::new(&ED25519, x)
                    .map_err(|_| WebAuthnError::PublicKey("invalid Ed25519 key"))?;
                Ok(Self::Ed25519(x))
            }
            (Some(3), Some(RS256)) => {
                let (n, e) = (bytes(map_get(m, -1)), bytes(map_get(m, -2)));
                let (Some(n), Some(e)) = (n, e) else {
                    return Err(WebAuthnError::PublicKey("missing n or e"));
                };
                let n = n
                    .iter()
                    .skip_while(|b| **b == 0)
                    .copied()
                    .collect::<Vec<_>>();
                if n.len() * 8 < 2048 || e.is_empty() || e.len() > 8 {
                    return Err(WebAuthnError::PublicKey("RSA keys must be ≥ 2048 bits"));
                }
                Ok(Self::Rs256 { n, e: e.to_vec() })
            }
            _ => Err(WebAuthnError::PublicKey(
                "unsupported key type or algorithm",
            )),
        }
    }

    #[must_use]
    pub fn alg(&self) -> i64 {
        match self {
            Self::Es256(_) => ES256,
            Self::Ed25519(_) => EDDSA,
            Self::Rs256 { .. } => RS256,
        }
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), WebAuthnError> {
        let ok = match self {
            Self::Es256(point) => ParsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point)
                .is_ok_and(|k| k.verify_sig(message, signature).is_ok()),
            Self::Ed25519(x) => ParsedPublicKey::new(&ED25519, x)
                .is_ok_and(|k| k.verify_sig(message, signature).is_ok()),
            Self::Rs256 { n, e } => RsaPublicKeyComponents { n, e }
                .verify(&RSA_PKCS1_2048_8192_SHA256, message, signature)
                .is_ok(),
        };
        if ok {
            Ok(())
        } else {
            Err(WebAuthnError::Signature)
        }
    }
}

/// Parsed authenticator data (§6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    pub attested: Option<AttestedCredential>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedCredential {
    pub aaguid: [u8; 16],
    pub credential_id: Vec<u8>,
    pub public_key: CoseKey,
    /// The COSE_Key bytes exactly as the authenticator produced them.
    pub public_key_cose: Vec<u8>,
}

impl AuthenticatorData {
    /// Parses authData, consuming every byte: attested credential data and
    /// extensions are decoded only as far as the flags announce them, and any
    /// trailing byte is an error.
    pub fn parse(data: &[u8]) -> Result<Self, WebAuthnError> {
        if data.len() < 37 {
            return Err(WebAuthnError::AuthData("shorter than 37 bytes"));
        }
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&data[..32]);
        let flags = data[32];
        let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
        if flags & FLAG_BS != 0 && flags & FLAG_BE == 0 {
            return Err(WebAuthnError::AuthData(
                "backup state without backup eligibility",
            ));
        }
        let mut rest = &data[37..];
        let attested = if flags & FLAG_AT != 0 {
            if rest.len() < 18 {
                return Err(WebAuthnError::AuthData(
                    "truncated attested credential data",
                ));
            }
            let mut aaguid = [0u8; 16];
            aaguid.copy_from_slice(&rest[..16]);
            let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
            if id_len == 0 || id_len > MAX_CREDENTIAL_ID || rest.len() < 18 + id_len {
                return Err(WebAuthnError::AuthData("bad credential id length"));
            }
            let credential_id = rest[18..18 + id_len].to_vec();
            rest = &rest[18 + id_len..];
            let mut cursor = Cursor::new(rest);
            let key: Value = ciborium::from_reader(&mut cursor)
                .map_err(|_| WebAuthnError::AuthData("credential public key is not CBOR"))?;
            let used = usize::try_from(cursor.position())
                .map_err(|_| WebAuthnError::AuthData("length"))?;
            let public_key_cose = rest[..used].to_vec();
            rest = &rest[used..];
            Some(AttestedCredential {
                aaguid,
                credential_id,
                public_key: CoseKey::from_value(&key)?,
                public_key_cose,
            })
        } else {
            None
        };
        if flags & FLAG_ED != 0 {
            // Authenticator extension outputs: decoded to find their end, then ignored.
            let mut cursor = Cursor::new(rest);
            let ext: Value = ciborium::from_reader(&mut cursor)
                .map_err(|_| WebAuthnError::AuthData("extensions are not CBOR"))?;
            if ext.as_map().is_none() {
                return Err(WebAuthnError::AuthData("extensions must be a map"));
            }
            let used = usize::try_from(cursor.position())
                .map_err(|_| WebAuthnError::AuthData("length"))?;
            rest = &rest[used..];
        }
        if !rest.is_empty() {
            return Err(WebAuthnError::AuthData("trailing bytes"));
        }
        Ok(Self {
            rp_id_hash,
            flags,
            sign_count,
            attested,
        })
    }

    #[must_use]
    pub fn user_verified(&self) -> bool {
        self.flags & FLAG_UP != 0 && self.flags & FLAG_UV != 0
    }

    #[must_use]
    pub fn backup_eligible(&self) -> bool {
        self.flags & FLAG_BE != 0
    }

    #[must_use]
    pub fn backed_up(&self) -> bool {
        self.flags & FLAG_BS != 0
    }
}

/// A verified new credential, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredential {
    pub credential_id: Vec<u8>,
    pub public_key_cose: Vec<u8>,
    pub alg: i64,
    pub sign_count: u32,
    pub aaguid: [u8; 16],
    pub backup_eligible: bool,
    pub backed_up: bool,
}

/// §7.1: verifies a `navigator.credentials.create()` response.
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: &[u8],
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<NewCredential, WebAuthnError> {
    verify_client_data(rp, client_data_json, "webauthn.create", challenge)?;
    let att: Value = ciborium::from_reader(attestation_object)
        .map_err(|_| WebAuthnError::Attestation("not CBOR"))?;
    let m = att
        .as_map()
        .ok_or(WebAuthnError::Attestation("not a map"))?;
    let field = |name: &str| {
        m.iter()
            .find(|(k, _)| k.as_text() == Some(name))
            .map(|(_, v)| v)
    };
    if field("fmt").and_then(Value::as_text) != Some("none") {
        return Err(WebAuthnError::Attestation(
            "only the \"none\" format is accepted",
        ));
    }
    if field("attStmt")
        .and_then(Value::as_map)
        .is_none_or(|s| !s.is_empty())
    {
        return Err(WebAuthnError::Attestation(
            "\"none\" requires an empty attStmt",
        ));
    }
    let auth_data = field("authData")
        .and_then(Value::as_bytes)
        .ok_or(WebAuthnError::Attestation("missing authData"))?;
    let ad = AuthenticatorData::parse(auth_data)?;
    if ad.rp_id_hash != rp.id_hash() {
        return Err(WebAuthnError::AuthData(
            "credential is scoped to another RP ID",
        ));
    }
    if !ad.user_verified() {
        return Err(WebAuthnError::UserVerification);
    }
    let cred = ad
        .attested
        .ok_or(WebAuthnError::AuthData("no attested credential data"))?;
    Ok(NewCredential {
        alg: cred.public_key.alg(),
        credential_id: cred.credential_id,
        public_key_cose: cred.public_key_cose,
        sign_count: ad.sign_count,
        aaguid: cred.aaguid,
        backup_eligible: ad.flags & FLAG_BE != 0,
        backed_up: ad.flags & FLAG_BS != 0,
    })
}

/// What a successful assertion changes on the stored credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssertionOutcome {
    pub sign_count: u32,
    pub backed_up: bool,
}

/// §7.2: verifies a `navigator.credentials.get()` response against the
/// stored credential.
pub fn verify_assertion(
    rp: &RelyingParty,
    challenge: &[u8],
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
    stored_key_cose: &[u8],
    stored_sign_count: u32,
) -> Result<AssertionOutcome, WebAuthnError> {
    let client_hash = verify_client_data(rp, client_data_json, "webauthn.get", challenge)?;
    let ad = AuthenticatorData::parse(authenticator_data)?;
    if ad.attested.is_some() {
        return Err(WebAuthnError::AuthData(
            "assertions carry no attested credential",
        ));
    }
    if ad.rp_id_hash != rp.id_hash() {
        return Err(WebAuthnError::AuthData(
            "credential is scoped to another RP ID",
        ));
    }
    if !ad.user_verified() {
        return Err(WebAuthnError::UserVerification);
    }
    let key = CoseKey::from_cbor(stored_key_cose)?;
    let message = [authenticator_data, &client_hash[..]].concat();
    key.verify(&message, signature)?;
    // Synced passkeys report 0 forever; a counter is only meaningful when
    // either side has ever been non-zero.
    if (ad.sign_count != 0 || stored_sign_count != 0) && ad.sign_count <= stored_sign_count {
        return Err(WebAuthnError::Counter);
    }
    Ok(AssertionOutcome {
        sign_count: ad.sign_count,
        backed_up: ad.flags & FLAG_BS != 0,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::jose::b64url;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::rsa::KeySize;
    use aws_lc_rs::signature::{
        ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair, RSA_PKCS1_SHA256,
        RsaKeyPair,
    };

    const RP_ID: &str = "read.sanad.test";
    const ORIGIN: &str = "https://read.sanad.test";
    const CHALLENGE: [u8; 32] = [9; 32];

    fn rp() -> RelyingParty {
        RelyingParty {
            id: RP_ID.into(),
            name: "Sanad".into(),
            origins: vec![ORIGIN.into()],
        }
    }

    fn cbor(v: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        ciborium::into_writer(v, &mut out).unwrap();
        out
    }

    fn i(v: i64) -> Value {
        Value::Integer(v.into())
    }

    enum Key {
        Es(EcdsaKeyPair),
        Ed(Ed25519KeyPair),
        Rsa(RsaKeyPair),
    }

    /// A software authenticator producing what browsers deliver.
    struct Authenticator {
        key: Key,
        id: Vec<u8>,
        count: u32,
    }

    impl Authenticator {
        fn new(alg: i64) -> Self {
            let key = match alg {
                ES256 => Key::Es(EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap()),
                EDDSA => Key::Ed(Ed25519KeyPair::generate().unwrap()),
                _ => Key::Rsa(RsaKeyPair::generate(KeySize::Rsa2048).unwrap()),
            };
            Self {
                key,
                id: vec![0xC5; 32],
                count: 0,
            }
        }

        fn cose(&self) -> Vec<u8> {
            let map = match &self.key {
                Key::Es(k) => {
                    let p = k.public_key().as_ref();
                    vec![
                        (i(1), i(2)),
                        (i(3), i(ES256)),
                        (i(-1), i(1)),
                        (i(-2), Value::Bytes(p[1..33].to_vec())),
                        (i(-3), Value::Bytes(p[33..].to_vec())),
                    ]
                }
                Key::Ed(k) => vec![
                    (i(1), i(1)),
                    (i(3), i(EDDSA)),
                    (i(-1), i(6)),
                    (i(-2), Value::Bytes(k.public_key().as_ref().to_vec())),
                ],
                Key::Rsa(k) => {
                    let p = k.public_key();
                    vec![
                        (i(1), i(3)),
                        (i(3), i(RS256)),
                        (
                            i(-1),
                            Value::Bytes(p.modulus().big_endian_without_leading_zero().to_vec()),
                        ),
                        (
                            i(-2),
                            Value::Bytes(p.exponent().big_endian_without_leading_zero().to_vec()),
                        ),
                    ]
                }
            };
            cbor(&Value::Map(map))
        }

        fn auth_data(&self, rp_id: &str, flags: u8, attested: bool) -> Vec<u8> {
            let mut d = digest(&SHA256, rp_id.as_bytes()).as_ref().to_vec();
            d.push(flags | if attested { FLAG_AT } else { 0 });
            d.extend_from_slice(&self.count.to_be_bytes());
            if attested {
                d.extend_from_slice(&[0xAA; 16]);
                d.extend_from_slice(&u16::try_from(self.id.len()).unwrap().to_be_bytes());
                d.extend_from_slice(&self.id);
                d.extend_from_slice(&self.cose());
            }
            d
        }

        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            let rng = SystemRandom::new();
            match &self.key {
                Key::Es(k) => k.sign(&rng, msg).unwrap().as_ref().to_vec(),
                Key::Ed(k) => k.sign(msg).as_ref().to_vec(),
                Key::Rsa(k) => {
                    let mut sig = vec![0u8; k.public_modulus_len()];
                    k.sign(&RSA_PKCS1_SHA256, &rng, msg, &mut sig).unwrap();
                    sig
                }
            }
        }

        fn attestation(auth_data: Vec<u8>) -> Vec<u8> {
            cbor(&Value::Map(vec![
                (Value::Text("fmt".into()), Value::Text("none".into())),
                (Value::Text("attStmt".into()), Value::Map(vec![])),
                (Value::Text("authData".into()), Value::Bytes(auth_data)),
            ]))
        }

        fn assert(&mut self, client: &[u8], flags: u8) -> (Vec<u8>, Vec<u8>) {
            let ad = self.auth_data(RP_ID, flags, false);
            let msg = [&ad[..], digest(&SHA256, client).as_ref()].concat();
            (ad.clone(), self.sign(&msg))
        }
    }

    fn client(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
        serde_json::json!({ "type": kind, "challenge": b64url(challenge), "origin": origin, "crossOrigin": false })
            .to_string()
            .into_bytes()
    }

    const OK: u8 = FLAG_UP | FLAG_UV;

    #[test]
    fn registers_and_authenticates_every_supported_algorithm() {
        for alg in SUPPORTED_ALGS {
            let mut a = Authenticator::new(alg);
            let att = Authenticator::attestation(a.auth_data(RP_ID, OK | FLAG_BE | FLAG_BS, true));
            let cred = verify_registration(
                &rp(),
                &CHALLENGE,
                &client("webauthn.create", &CHALLENGE, ORIGIN),
                &att,
            )
            .unwrap();
            assert_eq!(cred.alg, alg);
            assert_eq!(cred.credential_id, a.id);
            assert!(cred.backup_eligible && cred.backed_up);

            let c = client("webauthn.get", &CHALLENGE, ORIGIN);
            let (ad, sig) = a.assert(&c, OK);
            assert_eq!(
                verify_assertion(&rp(), &CHALLENGE, &c, &ad, &sig, &cred.public_key_cose, 0)
                    .unwrap()
                    .sign_count,
                0
            );
            let mut bad = sig.clone();
            let last = bad.len() - 1;
            bad[last] ^= 1;
            assert_eq!(
                verify_assertion(&rp(), &CHALLENGE, &c, &ad, &bad, &cred.public_key_cose, 0),
                Err(WebAuthnError::Signature),
                "alg {alg}"
            );
        }
    }

    #[test]
    fn client_data_is_bound_to_type_challenge_and_origin() {
        let a = Authenticator::new(ES256);
        let att = Authenticator::attestation(a.auth_data(RP_ID, OK, true));
        let reg = |c: Vec<u8>| verify_registration(&rp(), &CHALLENGE, &c, &att);
        assert_eq!(
            reg(client("webauthn.get", &CHALLENGE, ORIGIN)),
            Err(WebAuthnError::ClientData("wrong ceremony type"))
        );
        assert_eq!(
            reg(client("webauthn.create", &[8; 32], ORIGIN)),
            Err(WebAuthnError::ClientData("challenge mismatch"))
        );
        assert_eq!(
            reg(client("webauthn.create", &CHALLENGE, "https://evil.test")),
            Err(WebAuthnError::ClientData("origin not allowed"))
        );
        let cross = serde_json::json!({ "type": "webauthn.create", "challenge": b64url(&CHALLENGE), "origin": ORIGIN, "crossOrigin": true });
        assert_eq!(
            reg(cross.to_string().into_bytes()),
            Err(WebAuthnError::ClientData(
                "cross-origin ceremonies are not allowed"
            ))
        );
    }

    #[test]
    fn authenticator_data_rules() {
        let a = Authenticator::new(ES256);
        let c = client("webauthn.create", &CHALLENGE, ORIGIN);
        let reg = |ad: Vec<u8>| {
            verify_registration(&rp(), &CHALLENGE, &c, &Authenticator::attestation(ad))
        };
        assert_eq!(
            reg(a.auth_data("evil.test", OK, true)),
            Err(WebAuthnError::AuthData(
                "credential is scoped to another RP ID"
            ))
        );
        assert_eq!(
            reg(a.auth_data(RP_ID, FLAG_UP, true)),
            Err(WebAuthnError::UserVerification),
            "presence without verification"
        );
        assert_eq!(
            reg(a.auth_data(RP_ID, OK | FLAG_BS, true)),
            Err(WebAuthnError::AuthData(
                "backup state without backup eligibility"
            ))
        );
        let mut trailing = a.auth_data(RP_ID, OK, true);
        trailing.push(0);
        assert_eq!(
            reg(trailing),
            Err(WebAuthnError::AuthData("trailing bytes"))
        );
        assert_eq!(
            reg(a.auth_data(RP_ID, OK, false)),
            Err(WebAuthnError::AuthData("no attested credential data"))
        );
        // Extensions announced by ED are skipped, not treated as trailing garbage.
        let mut ext = a.auth_data(RP_ID, OK | FLAG_ED, true);
        ext.extend_from_slice(&cbor(&Value::Map(vec![(
            Value::Text("credProtect".into()),
            i(2),
        )])));
        assert!(reg(ext).is_ok());
    }

    #[test]
    fn only_none_attestation_is_accepted() {
        let a = Authenticator::new(ES256);
        let c = client("webauthn.create", &CHALLENGE, ORIGIN);
        let packed = cbor(&Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text("packed".into())),
            (
                Value::Text("attStmt".into()),
                Value::Map(vec![(Value::Text("alg".into()), i(-7))]),
            ),
            (
                Value::Text("authData".into()),
                Value::Bytes(a.auth_data(RP_ID, OK, true)),
            ),
        ]));
        assert_eq!(
            verify_registration(&rp(), &CHALLENGE, &c, &packed),
            Err(WebAuthnError::Attestation(
                "only the \"none\" format is accepted"
            ))
        );
    }

    #[test]
    fn counters_must_increase_unless_both_are_zero() {
        let mut a = Authenticator::new(ES256);
        let cose = a.cose();
        let c = client("webauthn.get", &CHALLENGE, ORIGIN);
        a.count = 7;
        let (ad, sig) = a.assert(&c, OK);
        assert_eq!(
            verify_assertion(&rp(), &CHALLENGE, &c, &ad, &sig, &cose, 5)
                .unwrap()
                .sign_count,
            7
        );
        assert_eq!(
            verify_assertion(&rp(), &CHALLENGE, &c, &ad, &sig, &cose, 7),
            Err(WebAuthnError::Counter),
            "replayed or cloned"
        );
        a.count = 0;
        let (ad, sig) = a.assert(&c, OK);
        assert!(
            verify_assertion(&rp(), &CHALLENGE, &c, &ad, &sig, &cose, 0).is_ok(),
            "synced passkeys stay at 0"
        );
    }

    #[test]
    fn rejects_weak_or_malformed_keys() {
        let short_rsa = cbor(&Value::Map(vec![
            (i(1), i(3)),
            (i(3), i(RS256)),
            (i(-1), Value::Bytes(vec![0xFF; 128])),
            (i(-2), Value::Bytes(vec![1, 0, 1])),
        ]));
        assert_eq!(
            CoseKey::from_cbor(&short_rsa),
            Err(WebAuthnError::PublicKey("RSA keys must be ≥ 2048 bits"))
        );
        let off_curve = cbor(&Value::Map(vec![
            (i(1), i(2)),
            (i(3), i(ES256)),
            (i(-1), i(1)),
            (i(-2), Value::Bytes(vec![1; 32])),
            (i(-3), Value::Bytes(vec![2; 32])),
        ]));
        assert_eq!(
            CoseKey::from_cbor(&off_curve),
            Err(WebAuthnError::PublicKey("point is not on P-256"))
        );
        let es384 = cbor(&Value::Map(vec![
            (i(1), i(2)),
            (i(3), i(-35)),
            (i(-1), i(2)),
        ]));
        assert_eq!(
            CoseKey::from_cbor(&es384),
            Err(WebAuthnError::PublicKey(
                "unsupported key type or algorithm"
            ))
        );
    }
}
