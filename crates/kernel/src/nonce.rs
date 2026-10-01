//! Server-provided DPoP nonces (RFC 9449 §8–9), stateless.
//!
//! `nonce = base64url( bucket_be64 ‖ HMAC-SHA256(secret, LABEL ‖ bucket_be64)[..16] )`
//! with `bucket = ⌊now / window⌋`. A nonce is accepted in its own bucket and
//! the next one, so every nonce lives between one and two windows. Any Kernel
//! replica can issue and check nonces without shared state, and rotating the
//! secret invalidates all outstanding nonces at once.

use aws_lc_rs::hmac;
use zeroize::Zeroizing;

use crate::jose::{b64url, b64url_decode};

const LABEL: &[u8] = b"sanad/dpop-nonce/v1";
const TAG_LEN: usize = 16;

#[derive(Debug)]
pub struct NonceManager {
    key: hmac::Key,
    window_secs: i64,
}

impl NonceManager {
    #[must_use]
    pub fn new(secret: &Zeroizing<[u8; 32]>, window_secs: i64) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref()),
            window_secs: window_secs.max(1),
        }
    }

    fn tag(&self, bucket: i64) -> hmac::Tag {
        let mut ctx = hmac::Context::with_key(&self.key);
        ctx.update(LABEL);
        ctx.update(&bucket.to_be_bytes());
        ctx.sign()
    }

    #[must_use]
    pub fn issue(&self, now: i64) -> String {
        let bucket = now.div_euclid(self.window_secs);
        let mut raw = [0u8; 8 + TAG_LEN];
        raw[..8].copy_from_slice(&bucket.to_be_bytes());
        raw[8..].copy_from_slice(&self.tag(bucket).as_ref()[..TAG_LEN]);
        b64url(&raw)
    }

    #[must_use]
    pub fn is_valid(&self, nonce: &str, now: i64) -> bool {
        let Ok(raw) = b64url_decode(nonce) else {
            return false;
        };
        if raw.len() != 8 + TAG_LEN {
            return false;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&raw[..8]);
        let bucket = i64::from_be_bytes(b);
        let current = now.div_euclid(self.window_secs);
        if bucket != current && bucket != current - 1 {
            return false;
        }
        let expected = self.tag(bucket);
        // Constant-time comparison of the truncated tag.
        aws_lc_rs::constant_time::verify_slices_are_equal(&raw[8..], &expected.as_ref()[..TAG_LEN])
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr(seed: u8) -> NonceManager {
        NonceManager::new(&Zeroizing::new([seed; 32]), 300)
    }

    #[test]
    fn valid_for_its_window_and_the_next() {
        let m = mgr(1);
        let n = m.issue(1_000_000);
        assert!(m.is_valid(&n, 1_000_000));
        assert!(m.is_valid(&n, 1_000_000 + 300));
        assert!(!m.is_valid(&n, 1_000_000 + 900));
        assert!(!m.is_valid(&n, 1_000_000 - 300), "future-dated nonce");
    }

    #[test]
    fn forged_or_foreign_nonces_fail() {
        let n = mgr(1).issue(5_000);
        assert!(!mgr(2).is_valid(&n, 5_000), "other secret");
        let mut raw = b64url_decode(&n).unwrap_or_default();
        raw[12] ^= 1;
        assert!(!mgr(1).is_valid(&b64url(&raw), 5_000));
        assert!(!mgr(1).is_valid("not-a-nonce", 5_000));
    }
}
