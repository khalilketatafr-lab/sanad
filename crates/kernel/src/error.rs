//! API errors with the response shapes RFC 9449 prescribes.
//!
//! The Kernel plays both DPoP roles:
//! - **authorization server** when it issues tokens (`POST /kernel/v1/devices`):
//!   errors are `400 {"error": "use_dpop_nonce" | "invalid_dpop_proof"}` (§8);
//! - **resource server** on protected routes: errors are `401` with
//!   `WWW-Authenticate: DPoP error="…", algs="ES256"` (§7.1, §9).
//!
//! Every DPoP-related response carries a fresh `DPoP-Nonce`.

use axum::Json;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub const DPOP_NONCE: HeaderName = HeaderName::from_static("dpop-nonce");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Token issuance (registration).
    AuthorizationServer,
    /// Protected resources.
    ResourceServer,
}

#[derive(Debug)]
pub enum ApiError {
    UseDpopNonce {
        role: Role,
        nonce: String,
    },
    InvalidDpopProof {
        role: Role,
        detail: String,
        nonce: String,
    },
    InvalidToken {
        detail: &'static str,
        nonce: Option<String>,
    },
    BadRequest(String),
    Forbidden(&'static str),
    NotFound(&'static str),
    Unavailable(&'static str),
    Internal,
}

fn www_authenticate(error: &str, detail: &str) -> HeaderValue {
    // Quoted-string: strip characters that would break the header syntax.
    let clean: String = detail
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && !c.is_control())
        .collect();
    HeaderValue::from_str(&format!(
        r#"DPoP error="{error}", error_description="{clean}", algs="ES256""#
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(r#"DPoP algs="ES256""#))
}

fn body(error: &str, description: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "error": error, "error_description": description }))
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        let mut with_nonce = |n: &str| {
            if let Ok(v) = HeaderValue::from_str(n) {
                headers.insert(DPOP_NONCE, v);
            }
        };
        let (status, json) = match &self {
            Self::UseDpopNonce { role, nonce } => {
                with_nonce(nonce);
                let msg = "Authorization server requires nonce in DPoP proof";
                match role {
                    Role::AuthorizationServer => {
                        (StatusCode::BAD_REQUEST, body("use_dpop_nonce", msg))
                    }
                    Role::ResourceServer => {
                        headers.insert(
                            header::WWW_AUTHENTICATE,
                            www_authenticate("use_dpop_nonce", msg),
                        );
                        (StatusCode::UNAUTHORIZED, body("use_dpop_nonce", msg))
                    }
                }
            }
            Self::InvalidDpopProof {
                role,
                detail,
                nonce,
            } => {
                with_nonce(nonce);
                match role {
                    Role::AuthorizationServer => {
                        (StatusCode::BAD_REQUEST, body("invalid_dpop_proof", detail))
                    }
                    Role::ResourceServer => {
                        headers.insert(
                            header::WWW_AUTHENTICATE,
                            www_authenticate("invalid_dpop_proof", detail),
                        );
                        (StatusCode::UNAUTHORIZED, body("invalid_dpop_proof", detail))
                    }
                }
            }
            Self::InvalidToken { detail, nonce } => {
                if let Some(n) = nonce {
                    with_nonce(n);
                }
                headers.insert(
                    header::WWW_AUTHENTICATE,
                    www_authenticate("invalid_token", detail),
                );
                (StatusCode::UNAUTHORIZED, body("invalid_token", detail))
            }
            Self::BadRequest(detail) => (StatusCode::BAD_REQUEST, body("invalid_request", detail)),
            Self::Forbidden(detail) => (StatusCode::FORBIDDEN, body("access_denied", detail)),
            Self::NotFound(detail) => (StatusCode::NOT_FOUND, body("not_found", detail)),
            Self::Unavailable(detail) => {
                headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    body("temporarily_unavailable", detail),
                )
            }
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                body("server_error", "internal error"),
            ),
        };
        (status, headers, json).into_response()
    }
}
