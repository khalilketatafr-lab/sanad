//! Entitlement authorization (blueprint 01 §2, 06 §3 "Cedar policies v1").
//!
//! The Kernel decides who may `open` an edition from data it owns: the
//! entitlement kind, the number of active sessions and devices, and the
//! requested chapter. The client never supplies any of this — it is counted
//! by the entitlement/session store under a lock and passed here as
//! [`OpenContext`]. Cedar evaluates the request against
//! `policies/sanad.cedar`; a `forbid` always overrides a `permit`, so a cap
//! can never be opened by a future publisher `permit`.
//!
//! Phase 1a policy: anonymous devices may sample the first chapters of a free
//! book (tighter window, no offline, set elsewhere); authenticated readers may
//! open free editions in full; at most 6 active devices and 2 concurrent
//! sessions per edition.

use std::sync::Arc;

use cedar_policy::{Authorizer, Context, Entities, EntityUid, PolicySet, Request};
use serde_json::json;
use thiserror::Error;

/// Chapters an anonymous device may sample (blueprint 01 §2; mirrors
/// `context.chapter < 3` in the policy).
pub const SAMPLE_CHAPTERS: u32 = 3;
/// Concurrent reading sessions per edition.
pub const MAX_CONCURRENT_SESSIONS: u32 = 2;
/// Active devices per account.
pub const MAX_DEVICES: u32 = 6;

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("policy set is not valid Cedar: {0}")]
    Parse(String),
    #[error("request could not be built: {0}")]
    Request(String),
    /// Not an error condition in the code sense: the request is simply denied.
    #[error("access denied by policy")]
    Denied,
}

/// The six entitlement kinds (blueprint 01 §2; the SQL `entitlement_kind`
/// enum). Only `free` and `sample` are active in Phase 1a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entitlement {
    Free,
    Sample,
    Purchase,
    Subscription,
    Rental,
    LibraryLoan,
}

impl Entitlement {
    /// The spelling the Cedar policy matches on.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Sample => "sample",
            Self::Purchase => "purchase",
            Self::Subscription => "subscription",
            Self::Rental => "rental",
            Self::LibraryLoan => "library_loan",
        }
    }
}

/// Who is asking. Anonymous sampling uses the shared `Device::"anonymous"`
/// principal; an authenticated reader is `User::"<uid>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    Anonymous,
    User(String),
}

/// Counters the Kernel computed under lock, plus the requested chapter.
/// Everything here is Kernel-owned; none of it is taken from the request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenContext {
    pub entitlement: Entitlement,
    /// Active sessions for this account on this edition, *including* the one
    /// being requested (prospective count).
    pub active_sessions: u32,
    /// Active devices for this account.
    pub active_devices: u32,
    /// 0-based chapter index being opened (for the sample window).
    pub chapter: u32,
}

/// A compiled policy set and authorizer. Cheap to clone (shared).
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    authorizer: Authorizer,
    policies: PolicySet,
    entities: Entities,
}

impl PolicyEngine {
    /// Compiles the bundled policy set. Fails only if the committed
    /// `sanad.cedar` is malformed, which a test catches.
    pub fn new() -> Result<Self, PolicyError> {
        let policies: PolicySet = include_str!("../policies/sanad.cedar")
            .parse()
            .map_err(|e: cedar_policy::ParseErrors| PolicyError::Parse(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                authorizer: Authorizer::new(),
                policies,
                entities: Entities::empty(),
            }),
        })
    }

    fn uid(ty: &str, id: &str) -> Result<EntityUid, PolicyError> {
        let ty = ty
            .parse()
            .map_err(|e: cedar_policy::ParseErrors| PolicyError::Request(e.to_string()))?;
        let id = cedar_policy::EntityId::new(id);
        Ok(EntityUid::from_type_name_and_id(ty, id))
    }

    /// Decides whether `principal` may open `edition_id` under `ctx`.
    /// `Ok(())` means allowed; `Err(PolicyError::Denied)` means the policy
    /// denied it (the normal "sign in / upsell" path, not a fault).
    pub fn evaluate_open(
        &self,
        principal: &Principal,
        edition_id: &str,
        ctx: &OpenContext,
    ) -> Result<(), PolicyError> {
        let principal_uid = match principal {
            Principal::Anonymous => Self::uid("Device", "anonymous")?,
            Principal::User(uid) => Self::uid("User", uid)?,
        };
        let action = Self::uid("Action", "open")?;
        let resource = Self::uid("Edition", edition_id)?;
        let context = Context::from_json_value(
            json!({
                "entitlement": ctx.entitlement.as_str(),
                "chapter": ctx.chapter,
                "active_sessions": ctx.active_sessions,
                "active_devices": ctx.active_devices,
            }),
            None,
        )
        .map_err(|e| PolicyError::Request(e.to_string()))?;
        let request = Request::new(principal_uid, action, resource, context, None)
            .map_err(|e| PolicyError::Request(e.to_string()))?;

        let response = self.inner.authorizer.is_authorized(
            &request,
            &self.inner.policies,
            &self.inner.entities,
        );
        if response.decision() == cedar_policy::Decision::Allow {
            Ok(())
        } else {
            tracing::info!(
                ?principal,
                edition = edition_id,
                entitlement = ctx.entitlement.as_str(),
                active_sessions = ctx.active_sessions,
                active_devices = ctx.active_devices,
                chapter = ctx.chapter,
                "open denied by policy"
            );
            Err(PolicyError::Denied)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn engine() -> PolicyEngine {
        PolicyEngine::new().unwrap()
    }

    fn ctx(entitlement: Entitlement, sessions: u32, devices: u32, chapter: u32) -> OpenContext {
        OpenContext {
            entitlement,
            active_sessions: sessions,
            active_devices: devices,
            chapter,
        }
    }

    fn allowed(p: &Principal, c: &OpenContext) -> bool {
        matches!(engine().evaluate_open(p, "ed-1", c), Ok(()))
    }

    #[test]
    fn bundled_policy_compiles() {
        PolicyEngine::new().unwrap();
    }

    #[test]
    fn anonymous_may_sample_the_first_chapters() {
        let anon = Principal::Anonymous;
        assert!(allowed(&anon, &ctx(Entitlement::Sample, 1, 0, 0)));
        assert!(allowed(&anon, &ctx(Entitlement::Sample, 1, 0, 2)));
        // The fourth chapter (index 3) is past the sample window.
        assert!(!allowed(&anon, &ctx(Entitlement::Sample, 1, 0, 3)));
        // Anonymous cannot open a free edition in full: that needs a user.
        assert!(!allowed(&anon, &ctx(Entitlement::Free, 1, 0, 0)));
    }

    #[test]
    fn users_open_free_editions_in_full() {
        let user = Principal::User("u-123".into());
        assert!(allowed(&user, &ctx(Entitlement::Free, 1, 1, 50)));
        assert!(allowed(&user, &ctx(Entitlement::Free, 1, 6, 0)));
        // Phase 1a has no purchase/subscription policy yet: denied, not error.
        assert!(!allowed(&user, &ctx(Entitlement::Purchase, 1, 1, 0)));
    }

    #[test]
    fn caps_forbid_overrides_permit() {
        let user = Principal::User("u-123".into());
        // A second concurrent session (prospective count 2) is forbidden.
        assert!(!allowed(&user, &ctx(Entitlement::Free, 2, 1, 0)));
        // A seventh device is forbidden.
        assert!(!allowed(&user, &ctx(Entitlement::Free, 1, 7, 0)));
        // The forbid applies to anonymous sampling too.
        assert!(!allowed(
            &Principal::Anonymous,
            &ctx(Entitlement::Sample, 2, 0, 0)
        ));
    }

    #[test]
    fn entitlement_spellings_match_the_sql_enum() {
        assert_eq!(Entitlement::LibraryLoan.as_str(), "library_loan");
        assert_eq!(Entitlement::Free.as_str(), "free");
    }
}
