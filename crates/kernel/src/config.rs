//! Kernel configuration from the environment.
//!
//! | Variable | Meaning |
//! |---|---|
//! | `SANAD_ENV` | `production` makes every secret mandatory |
//! | `SANAD_KERNEL_BIND` | listen address (default `127.0.0.1:8787`) |
//! | `SANAD_KERNEL_PUBLIC_ORIGIN` | external origin used for DPoP `htu` checks, e.g. `https://kernel.sanad.app` |
//! | `SANAD_READER_ORIGINS` | comma-separated CORS origins, e.g. `https://read.sanad.app`; also the only origins passkey ceremonies may run on |
//! | `SANAD_WEBAUTHN_RP_ID` | WebAuthn RP ID, e.g. `sanad.app` (default `localhost`); every reader origin's host must be it or a subdomain of it |
//! | `SANAD_KERNEL_TOKEN_KEY` | base64url Ed25519 secret key (64 bytes) for PASETO access tokens |
//! | `SANAD_KERNEL_NONCE_KEY` | base64url 32-byte HMAC key for DPoP nonces |
//! | `DATABASE_URL` | Postgres (required in production; in-memory store otherwise) |
//!
//! In production, secrets come from the platform's secret manager, never the repo.

use core::time::Duration;
use std::env;
use std::net::SocketAddr;

use thiserror::Error;
use url::Url;
use zeroize::Zeroizing;

use crate::jose::b64url_decode;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{0} is required when SANAD_ENV=production")]
    Missing(&'static str),
    #[error("{0} is invalid: {1}")]
    Invalid(&'static str, String),
}

#[derive(Debug)]
pub struct KernelConfig {
    pub production: bool,
    pub bind: SocketAddr,
    pub public_origin: Url,
    pub reader_origins: Vec<String>,
    pub rp_id: String,
    pub token_key: Option<Zeroizing<Vec<u8>>>,
    pub nonce_key: Option<Zeroizing<[u8; 32]>>,
    pub database_url: Option<String>,
    pub token_ttl: Duration,
    pub nonce_window_secs: i64,
    pub proof_skew_secs: i64,
}

impl KernelConfig {
    /// Defaults for tests and local development: in-memory, ephemeral keys.
    #[must_use]
    pub fn development(public_origin: Url) -> Self {
        Self {
            production: false,
            bind: SocketAddr::from(([127, 0, 0, 1], 8787)),
            public_origin,
            reader_origins: Vec::new(),
            rp_id: "localhost".into(),
            token_key: None,
            nonce_key: None,
            database_url: None,
            token_ttl: Duration::from_mins(15),
            nonce_window_secs: 300,
            proof_skew_secs: 60,
        }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let production = env::var("SANAD_ENV").is_ok_and(|v| v == "production");
        let get = |k: &'static str| env::var(k).ok().filter(|v| !v.is_empty());
        let require = |k: &'static str| -> Result<Option<String>, ConfigError> {
            match get(k) {
                None if production => Err(ConfigError::Missing(k)),
                v => Ok(v),
            }
        };

        let bind = get("SANAD_KERNEL_BIND")
            .unwrap_or_else(|| "127.0.0.1:8787".into())
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                ConfigError::Invalid("SANAD_KERNEL_BIND", e.to_string())
            })?;
        let origin =
            require("SANAD_KERNEL_PUBLIC_ORIGIN")?.unwrap_or_else(|| format!("http://{bind}"));
        let public_origin = Url::parse(&origin)
            .map_err(|e| ConfigError::Invalid("SANAD_KERNEL_PUBLIC_ORIGIN", e.to_string()))?;
        if public_origin.path() != "/" || public_origin.query().is_some() {
            return Err(ConfigError::Invalid(
                "SANAD_KERNEL_PUBLIC_ORIGIN",
                "must be a bare origin".into(),
            ));
        }
        if production && public_origin.scheme() != "https" {
            return Err(ConfigError::Invalid(
                "SANAD_KERNEL_PUBLIC_ORIGIN",
                "must be https in production".into(),
            ));
        }

        let token_key = require("SANAD_KERNEL_TOKEN_KEY")?
            .map(|v| {
                b64url_decode(&v)
                    .map(Zeroizing::new)
                    .map_err(|e| ConfigError::Invalid("SANAD_KERNEL_TOKEN_KEY", e.to_string()))
            })
            .transpose()?;
        let nonce_key = require("SANAD_KERNEL_NONCE_KEY")?
            .map(|v| {
                let bytes =
                    Zeroizing::new(b64url_decode(&v).map_err(|e| {
                        ConfigError::Invalid("SANAD_KERNEL_NONCE_KEY", e.to_string())
                    })?);
                let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                    ConfigError::Invalid("SANAD_KERNEL_NONCE_KEY", "need 32 bytes".into())
                })?;
                Ok::<_, ConfigError>(Zeroizing::new(arr))
            })
            .transpose()?;

        let reader_origins: Vec<String> = get("SANAD_READER_ORIGINS")
            .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
            .unwrap_or_default();
        let rp_id = require("SANAD_WEBAUTHN_RP_ID")?.unwrap_or_else(|| "localhost".into());
        for origin in &reader_origins {
            if !rp_id_covers(&rp_id, origin) {
                return Err(ConfigError::Invalid(
                    "SANAD_WEBAUTHN_RP_ID",
                    format!("{rp_id} is not a registrable suffix of {origin}"),
                ));
            }
        }

        Ok(Self {
            production,
            bind,
            public_origin,
            reader_origins,
            rp_id,
            token_key,
            nonce_key,
            database_url: require("DATABASE_URL")?,
            ..Self::development(
                Url::parse("http://localhost")
                    .map_err(|e| ConfigError::Invalid("default", e.to_string()))?,
            )
        })
    }
}

/// WebAuthn §5.1.4.1: an origin may use `rp_id` if its host equals it or is
/// a subdomain of it.
#[must_use]
pub fn rp_id_covers(rp_id: &str, origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| host == rp_id || host.ends_with(&format!(".{rp_id}")))
}

#[cfg(test)]
mod tests {
    use super::rp_id_covers;

    #[test]
    fn rp_id_must_be_the_host_or_a_parent_domain() {
        assert!(rp_id_covers("sanad.app", "https://read.sanad.app"));
        assert!(rp_id_covers("read.sanad.app", "https://read.sanad.app"));
        assert!(rp_id_covers("localhost", "http://localhost:5173"));
        assert!(!rp_id_covers("sanad.app", "https://evilsanad.app"));
        assert!(!rp_id_covers("read.sanad.app", "https://sanad.app"));
    }
}
