//! # Unified Security Kernel
//!
//! Phase 0 skeleton (spike S4): device registration with DPoP proof of
//! possession (RFC 9449), DPoP-bound PASETO access tokens, stateless nonces,
//! `jti` replay protection, the auth extractor every later route builds on,
//! passkey accounts (WebAuthn, see [`webauthn`]) that devices sign in to,
//! and stub leases: chunk keys ECDH-wrapped to the device, with a Reading
//! Capability Token.
//!
//! Spec: `docs/blueprint/01-architecture.md` §3.

pub mod accounts;
pub mod auth;
pub mod catalog;
pub mod config;
pub mod devices;
pub mod dpop;
pub mod error;
pub mod jose;
pub mod lease;
pub mod nonce;
pub mod policy;
pub mod replay;
pub mod routes;
pub mod token;
pub mod webauthn;

use core::time::Duration;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::routing::{get, post};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use url::Url;
use zeroize::Zeroizing;

use crate::accounts::AccountStore;
use crate::catalog::Catalog;
use crate::config::KernelConfig;
use crate::devices::DeviceStore;
use crate::dpop::DpopVerifier;
use crate::nonce::NonceManager;
use crate::replay::ReplayCache;
use crate::token::{CEREMONY_TTL, TokenError, TokenIssuer};
use crate::webauthn::RelyingParty;

/// Active devices per account (blueprint 01 §3.2).
pub const MAX_DEVICES_PER_USER: usize = 6;

/// Shared Kernel state.
#[derive(Debug)]
pub struct Kernel {
    pub config: KernelConfig,
    pub verifier: DpopVerifier,
    pub nonces: NonceManager,
    pub replay: ReplayCache,
    pub tokens: TokenIssuer,
    pub devices: DeviceStore,
    pub accounts: AccountStore,
    pub catalog: Catalog,
    /// Single use of passkey ceremony challenges, per device key.
    pub ceremonies: ReplayCache,
}

pub type AppState = Arc<Kernel>;

/// Seconds since the Unix epoch.
#[must_use]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Kernel {
    /// Builds the Kernel. Missing keys are generated (development only;
    /// `KernelConfig::from_env` refuses that in production).
    pub fn new(config: KernelConfig, devices: DeviceStore) -> Result<Self, TokenError> {
        let tokens = match &config.token_key {
            Some(k) => TokenIssuer::from_secret_bytes(k, config.token_ttl)?,
            None => TokenIssuer::generate(config.token_ttl)?,
        };
        let nonce_key = config.nonce_key.clone().unwrap_or_else(|| {
            let mut k = Zeroizing::new([0u8; 32]);
            let _ = aws_lc_rs::rand::fill(k.as_mut());
            k
        });
        Ok(Self {
            verifier: DpopVerifier {
                max_skew_secs: config.proof_skew_secs,
                ..DpopVerifier::default()
            },
            nonces: NonceManager::new(&nonce_key, config.nonce_window_secs),
            // Entries must outlive the iat acceptance window on both sides.
            replay: ReplayCache::new(2 * config.proof_skew_secs + 1, 1_000_000),
            tokens,
            accounts: AccountStore::alongside(&devices),
            devices,
            ceremonies: ReplayCache::new(
                CEREMONY_TTL.as_secs() as i64 + config.proof_skew_secs,
                100_000,
            ),
            // Production editions come from Postgres (KMS-wrapped TMKs); the
            // development catalog holds only the canary book under a test key.
            catalog: if config.production {
                Catalog::default()
            } else {
                Catalog::development()
            },
            config,
        })
    }

    /// The WebAuthn relying party: our RP ID; ceremonies may only run on the
    /// reader origins.
    #[must_use]
    pub fn relying_party(&self) -> RelyingParty {
        RelyingParty {
            id: self.config.rp_id.clone(),
            name: "Sanad".into(),
            origins: self.config.reader_origins.clone(),
        }
    }

    /// Canonical external URL of a request path, for `htu` comparison. The
    /// Kernel runs behind a proxy, so it never trusts Host headers for this.
    #[must_use]
    pub fn url_for(&self, path: &str) -> Option<Url> {
        self.config.public_origin.join(path).ok()
    }
}

pub fn router(state: AppState) -> Router {
    let dpop = HeaderName::from_static("dpop");
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(
            state
                .config
                .reader_origins
                .iter()
                .filter_map(|o| HeaderValue::from_str(o).ok()),
        ))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, dpop])
        .expose_headers([error::DPOP_NONCE, header::WWW_AUTHENTICATE])
        .max_age(Duration::from_mins(10));

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(routes::devices::PATH, post(routes::devices::register))
        .route(routes::devices::SELF_PATH, get(routes::devices::get_self))
        .route(routes::editions::PATH, post(routes::editions::open))
        .route(routes::passkeys::BEGIN, post(routes::passkeys::begin))
        .route(routes::passkeys::FINISH, post(routes::passkeys::finish))
        .layer(RequestBodyLimitLayer::new(16 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
