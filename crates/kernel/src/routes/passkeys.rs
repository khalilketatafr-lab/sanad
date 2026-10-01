//! `POST /kernel/v1/auth/passkey:begin` and `:finish`: passkey sign-up and
//! sign-in for the calling device (blueprint 01 §3.2).
//!
//! Both are DPoP-protected: a ceremony always runs *on a registered device*
//! and ends by attaching that device to the account, then minting a new
//! access token (`tier: member`, `uid`). The ceremony state is a signed token
//! bound to the device's key and accepted once, so replicas share nothing.
//!
//! - `register`: creates an account whose first passkey is the new
//!   credential, or adds a passkey when the device is already signed in.
//! - `authenticate`: discoverable-credential sign-in (no username).

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::accounts::AccountError;
use crate::auth::DpopAuth;
use crate::devices::StoreError;
use crate::error::{ApiError, DPOP_NONCE};
use crate::jose::{b64url, b64url_decode};
use crate::replay::Seen;
use crate::token::{AccessClaims, CEREMONY_TTL, CeremonyClaims, CeremonyMode};
use crate::webauthn::{SUPPORTED_ALGS, verify_assertion, verify_registration};
use crate::{AppState, Kernel, MAX_DEVICES_PER_USER, unix_now};

pub const BEGIN: &str = "/kernel/v1/auth/passkey:begin";
pub const FINISH: &str = "/kernel/v1/auth/passkey:finish";
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Register,
    Authenticate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginRequest {
    pub mode: Mode,
    /// Register only: the label the passkey manager shows (e.g. an email).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BeginResponse {
    pub ceremony: String,
    /// `PublicKeyCredentialCreationOptionsJSON` or `…RequestOptionsJSON`.
    #[serde(rename = "publicKey")]
    pub public_key: Value,
}

/// `PublicKeyCredential.toJSON()`; binary members are base64url.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialJson {
    pub raw_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub response: ResponseJson,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseJson {
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(default)]
    pub attestation_object: Option<String>,
    #[serde(default)]
    pub authenticator_data: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub user_handle: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishRequest {
    pub ceremony: String,
    pub credential: CredentialJson,
}

#[derive(Debug, Serialize)]
pub struct FinishResponse {
    pub user_id: Uuid,
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub dpop_jkt: String,
    pub credential_id: String,
}

fn headers(k: &Kernel) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&k.nonces.issue(unix_now())) {
        h.insert(DPOP_NONCE, v);
    }
    h
}

fn random<const N: usize>() -> Result<[u8; N], ApiError> {
    let mut b = [0u8; N];
    SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| ApiError::Internal)?;
    Ok(b)
}

fn decode(field: &'static str, v: &str) -> Result<Vec<u8>, ApiError> {
    b64url_decode(v).map_err(|_| ApiError::BadRequest(format!("{field}: not base64url")))
}

fn backend(e: AccountError) -> ApiError {
    match e {
        AccountError::CredentialExists => {
            ApiError::BadRequest("this passkey is already registered".into())
        }
        AccountError::Backend(msg) => {
            tracing::error!(error = %msg, "account store");
            ApiError::Internal
        }
    }
}

pub async fn begin(
    State(k): State<AppState>,
    auth: DpopAuth,
    Json(req): Json<BeginRequest>,
) -> Result<(HeaderMap, Json<BeginResponse>), ApiError> {
    let rp = k.relying_party();
    if rp.origins.is_empty() {
        return Err(ApiError::Unavailable(
            "passkeys are not configured (no reader origins)",
        ));
    }
    let challenge: [u8; 32] = random()?;
    let timeout_ms = CEREMONY_TTL.as_millis().saturating_sub(30_000);
    let (claims, public_key) = match req.mode {
        Mode::Register => {
            // A signed-in device adds a passkey to its account; otherwise a
            // new account is created with a fresh, opaque user handle.
            let (user_id, handle) = match auth.device.user_id {
                Some(uid) => {
                    let h = k.accounts.user_handle(uid).await.map_err(backend)?;
                    (uid, h.ok_or(ApiError::Internal)?)
                }
                None => (Uuid::now_v7(), random::<32>()?.to_vec()),
            };
            let name = req.name.unwrap_or_else(|| "Sanad reader".into());
            if name.trim().is_empty() || name.chars().count() > MAX_NAME {
                return Err(ApiError::BadRequest(format!(
                    "name must be 1–{MAX_NAME} characters"
                )));
            }
            let options = json!({
                "rp": { "id": rp.id, "name": rp.name },
                "user": { "id": b64url(&handle), "name": name, "displayName": name },
                "challenge": b64url(&challenge),
                "pubKeyCredParams": SUPPORTED_ALGS.iter().map(|a| json!({ "type": "public-key", "alg": a })).collect::<Vec<_>>(),
                "timeout": timeout_ms,
                "attestation": "none",
                "authenticatorSelection": {
                    "residentKey": "required",
                    "requireResidentKey": true,
                    "userVerification": "required",
                },
                "excludeCredentials": [],
            });
            let claims = CeremonyClaims {
                mode: CeremonyMode::Register,
                challenge,
                jkt: auth.proof.jkt.clone(),
                user_id: Some(user_id),
                user_handle: Some(handle),
            };
            (claims, options)
        }
        Mode::Authenticate => {
            let options = json!({
                "challenge": b64url(&challenge),
                "rpId": rp.id,
                "timeout": timeout_ms,
                "userVerification": "required",
                "allowCredentials": [],
            });
            let claims = CeremonyClaims {
                mode: CeremonyMode::Authenticate,
                challenge,
                jkt: auth.proof.jkt.clone(),
                user_id: None,
                user_handle: None,
            };
            (claims, options)
        }
    };
    let ceremony = k
        .tokens
        .issue_ceremony(&claims)
        .map_err(|_| ApiError::Internal)?;
    Ok((
        headers(&k),
        Json(BeginResponse {
            ceremony,
            public_key,
        }),
    ))
}

/// The verified inputs of `passkey:finish` that both modes share.
struct Finish<'a> {
    k: &'a Kernel,
    auth: &'a DpopAuth,
    ceremony: CeremonyClaims,
    cred: &'a CredentialJson,
    raw_id: Vec<u8>,
    client_data: Vec<u8>,
}

fn invalid(e: crate::webauthn::WebAuthnError) -> ApiError {
    ApiError::BadRequest(e.to_string())
}

impl Finish<'_> {
    /// §7.1, then store: a new account, or another passkey for this device's account.
    async fn register(&self) -> Result<Uuid, ApiError> {
        let att = self
            .cred
            .response
            .attestation_object
            .as_deref()
            .ok_or_else(|| ApiError::BadRequest("attestationObject required".into()))?;
        let new = verify_registration(
            &self.k.relying_party(),
            &self.ceremony.challenge,
            &self.client_data,
            &decode("attestationObject", att)?,
        )
        .map_err(invalid)?;
        if new.credential_id != self.raw_id {
            return Err(ApiError::BadRequest(
                "rawId does not match the attested credential".into(),
            ));
        }
        let (Some(uid), Some(handle)) =
            (self.ceremony.user_id, self.ceremony.user_handle.as_deref())
        else {
            return Err(ApiError::BadRequest("ceremony invalid or expired".into()));
        };
        if self.auth.device.user_id == Some(uid) {
            self.k
                .accounts
                .add_passkey(uid, &new)
                .await
                .map_err(backend)?;
        } else {
            self.k
                .accounts
                .create_user_with_passkey(uid, handle, &new)
                .await
                .map_err(backend)?;
        }
        tracing::info!(user = %uid, device = %self.auth.device.id, alg = new.alg, "passkey registered");
        Ok(uid)
    }

    /// §7.2 against the stored credential, then record its use.
    async fn authenticate(&self) -> Result<Uuid, ApiError> {
        let response = &self.cred.response;
        let (Some(ad), Some(sig)) = (&response.authenticator_data, &response.signature) else {
            return Err(ApiError::BadRequest(
                "authenticatorData and signature required".into(),
            ));
        };
        let stored = self
            .k
            .accounts
            .passkey(&self.raw_id)
            .await
            .map_err(backend)?
            .ok_or_else(|| ApiError::BadRequest("unknown passkey".into()))?;
        if let Some(h) = &response.user_handle
            && decode("userHandle", h)? != stored.user_handle
        {
            return Err(ApiError::BadRequest(
                "userHandle does not match the passkey".into(),
            ));
        }
        let outcome = verify_assertion(
            &self.k.relying_party(),
            &self.ceremony.challenge,
            &self.client_data,
            &decode("authenticatorData", ad)?,
            &decode("signature", sig)?,
            &stored.public_key_cose,
            stored.sign_count,
        )
        .map_err(invalid)?;
        self.k
            .accounts
            .record_use(&self.raw_id, outcome.sign_count, outcome.backed_up)
            .await
            .map_err(backend)?;
        tracing::info!(user = %stored.user_id, device = %self.auth.device.id, "passkey sign-in");
        Ok(stored.user_id)
    }
}

pub async fn finish(
    State(k): State<AppState>,
    auth: DpopAuth,
    Json(req): Json<FinishRequest>,
) -> Result<(HeaderMap, Json<FinishResponse>), ApiError> {
    let ceremony = k
        .tokens
        .verify_ceremony(&req.ceremony)
        .map_err(|_| ApiError::BadRequest("ceremony invalid or expired".into()))?;
    if ceremony.jkt != auth.proof.jkt {
        return Err(ApiError::Forbidden("ceremony belongs to another device"));
    }
    match k
        .ceremonies
        .check_and_insert(&ceremony.jkt, &b64url(&ceremony.challenge), unix_now())
    {
        Seen::First => {}
        Seen::Replay => return Err(ApiError::BadRequest("ceremony already used".into())),
        Seen::Overloaded => return Err(ApiError::Unavailable("ceremony cache saturated")),
    }
    let cred = &req.credential;
    if cred.kind != "public-key" {
        return Err(ApiError::BadRequest(
            "credential type must be public-key".into(),
        ));
    }
    let mode = ceremony.mode;
    let step = Finish {
        k: &k,
        auth: &auth,
        ceremony,
        cred,
        raw_id: decode("rawId", &cred.raw_id)?,
        client_data: decode("clientDataJSON", &cred.response.client_data_json)?,
    };
    let user_id = match mode {
        CeremonyMode::Register => step.register().await?,
        CeremonyMode::Authenticate => step.authenticate().await?,
    };
    let credential_id = b64url(&step.raw_id);

    k.devices
        .attach_user(auth.device.id, user_id, MAX_DEVICES_PER_USER)
        .await
        .map_err(|e| match e {
            StoreError::OtherUser => {
                ApiError::Forbidden("this device is signed in to another account")
            }
            StoreError::DeviceCap => {
                ApiError::Forbidden("device limit reached: sign out on another device first")
            }
            _ => ApiError::Internal,
        })?;
    let access_token = k
        .tokens
        .issue(&AccessClaims {
            device_id: auth.device.id,
            jkt: auth.proof.jkt.clone(),
            tier: "member".into(),
            user_id: Some(user_id),
        })
        .map_err(|_| ApiError::Internal)?;
    Ok((
        headers(&k),
        Json(FinishResponse {
            user_id,
            access_token,
            token_type: "DPoP",
            expires_in: k.tokens.ttl().as_secs(),
            dpop_jkt: auth.proof.jkt.clone(),
            credential_id,
        }),
    ))
}
