//! `jti` replay protection for DPoP proofs (RFC 9449 §11.1).
//!
//! Each proof may be used once. Entries only need to outlive the proof's
//! acceptance window (`iat` skew), after which the `iat` check rejects it
//! anyway. This in-process cache is correct for a single replica. Behind a
//! load balancer, the Valkey-backed implementation (`SET key NX EX ttl`)
//! replaces it, keeping the same interface.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug)]
pub struct ReplayCache {
    inner: Mutex<Inner>,
    ttl_secs: i64,
    max_entries: usize,
}

#[derive(Debug, Default)]
struct Inner {
    seen: HashMap<(String, String), i64>,
    last_sweep: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    First,
    Replay,
    /// Cache at capacity with nothing expired: fail closed.
    Overloaded,
}

impl ReplayCache {
    #[must_use]
    pub fn new(ttl_secs: i64, max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            ttl_secs,
            max_entries,
        }
    }

    /// Records `(jkt, jti)`; reports whether it was already seen.
    pub fn check_and_insert(&self, jkt: &str, jti: &str, now: i64) -> Seen {
        let Ok(mut inner) = self.inner.lock() else {
            return Seen::Overloaded;
        };
        if now - inner.last_sweep >= self.ttl_secs || inner.seen.len() >= self.max_entries {
            inner.seen.retain(|_, expires| *expires > now);
            inner.last_sweep = now;
        }
        let key = (jkt.to_owned(), jti.to_owned());
        if inner.seen.get(&key).is_some_and(|exp| *exp > now) {
            return Seen::Replay;
        }
        if inner.seen.len() >= self.max_entries {
            return Seen::Overloaded;
        }
        inner.seen.insert(key, now + self.ttl_secs);
        Seen::First
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_use_is_a_replay_until_expiry() {
        let c = ReplayCache::new(120, 10);
        assert_eq!(c.check_and_insert("k", "j1", 100), Seen::First);
        assert_eq!(c.check_and_insert("k", "j1", 150), Seen::Replay);
        assert_eq!(
            c.check_and_insert("other", "j1", 150),
            Seen::First,
            "scoped per key"
        );
        assert_eq!(
            c.check_and_insert("k", "j1", 300),
            Seen::First,
            "expired entry"
        );
    }

    #[test]
    fn fails_closed_at_capacity() {
        let c = ReplayCache::new(120, 2);
        assert_eq!(c.check_and_insert("k", "a", 0), Seen::First);
        assert_eq!(c.check_and_insert("k", "b", 0), Seen::First);
        assert_eq!(c.check_and_insert("k", "c", 0), Seen::Overloaded);
        assert_eq!(
            c.check_and_insert("k", "c", 200),
            Seen::First,
            "space reclaimed after expiry"
        );
    }
}
