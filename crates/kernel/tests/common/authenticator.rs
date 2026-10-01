//! A software passkey (ES256, discoverable, user-verifying) that produces
//! what `navigator.credentials.create()/get()` deliver, as `toJSON()` output.
#![allow(dead_code, clippy::unwrap_used, unreachable_pub)]

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use ciborium::Value;
use sanad_kernel::jose::{b64url, b64url_decode};
use serde_json::{Value as Json, json};

pub const READER: &str = "https://read.sanad.test";
pub const RP_ID: &str = "read.sanad.test";

pub struct Passkey {
    key: EcdsaKeyPair,
    pub id: Vec<u8>,
    pub user_handle: Vec<u8>,
    pub count: u32,
}

fn cbor(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).unwrap();
    out
}

fn client_data(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
    json!({ "type": kind, "challenge": challenge, "origin": origin, "crossOrigin": false })
        .to_string()
        .into_bytes()
}

fn auth_data(rp_id: &str, flags: u8, count: u32) -> Vec<u8> {
    let mut d = digest(&SHA256, rp_id.as_bytes()).as_ref().to_vec();
    d.push(flags);
    d.extend_from_slice(&count.to_be_bytes());
    d
}

impl Passkey {
    /// `navigator.credentials.create(options)` → `toJSON()`.
    pub fn create(options: &Json, origin: &str) -> (Self, Json) {
        let key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap();
        let id: Vec<u8> = digest(&SHA256, key.public_key().as_ref()).as_ref()[..16].to_vec();
        let user_handle = b64url_decode(options["user"]["id"].as_str().unwrap()).unwrap();
        let p = key.public_key().as_ref();
        let i = |v: i64| Value::Integer(v.into());
        let cose = cbor(&Value::Map(vec![
            (i(1), i(2)),
            (i(3), i(-7)),
            (i(-1), i(1)),
            (i(-2), Value::Bytes(p[1..33].to_vec())),
            (i(-3), Value::Bytes(p[33..].to_vec())),
        ]));
        let rp_id = options["rp"]["id"].as_str().unwrap();
        // UP | UV | BE | BS | AT: a synced, user-verifying platform passkey.
        let mut ad = auth_data(rp_id, 0b0101_1101, 0);
        ad.extend_from_slice(&[0; 16]);
        ad.extend_from_slice(&u16::try_from(id.len()).unwrap().to_be_bytes());
        ad.extend_from_slice(&id);
        ad.extend_from_slice(&cose);
        let att = cbor(&Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text("none".into())),
            (Value::Text("attStmt".into()), Value::Map(vec![])),
            (Value::Text("authData".into()), Value::Bytes(ad)),
        ]));
        let cd = client_data(
            "webauthn.create",
            options["challenge"].as_str().unwrap(),
            origin,
        );
        let json = json!({
            "id": b64url(&id),
            "rawId": b64url(&id),
            "type": "public-key",
            "response": { "clientDataJSON": b64url(&cd), "attestationObject": b64url(&att), "transports": ["internal"] },
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {},
        });
        (
            Self {
                key,
                id,
                user_handle,
                count: 0,
            },
            json,
        )
    }

    /// `navigator.credentials.get(options)` → `toJSON()`.
    pub fn get(&self, options: &Json, origin: &str) -> Json {
        let ad = auth_data(options["rpId"].as_str().unwrap(), 0b0001_1101, self.count);
        let cd = client_data(
            "webauthn.get",
            options["challenge"].as_str().unwrap(),
            origin,
        );
        let msg = [&ad[..], digest(&SHA256, &cd).as_ref()].concat();
        let sig = self.key.sign(&SystemRandom::new(), &msg).unwrap();
        json!({
            "id": b64url(&self.id),
            "rawId": b64url(&self.id),
            "type": "public-key",
            "response": {
                "clientDataJSON": b64url(&cd),
                "authenticatorData": b64url(&ad),
                "signature": b64url(sig.as_ref()),
                "userHandle": b64url(&self.user_handle),
            },
            "clientExtensionResults": {},
        })
    }
}
