//! Reading-session store (blueprint 05 §5): one active session per device,
//! holding the 64-bit session id that seeds the Ex Libris watermark and the
//! Sentinel risk state. On every lease open or renewal the Kernel folds a
//! behavioural digest through the session's [`Sentinel`] and applies the
//! returned [`Response`] (prefetch window, attestation, pacing, pause).
//!
//! The store mirrors [`crate::devices::DeviceStore`]: an in-memory map for
//! development and tests, and a Postgres table for production. The Postgres
//! path serializes concurrent opens for the same device with a transaction-
//! scoped advisory lock, so a device cannot race two sessions into existence.
//! No content is stored — only aggregates and the risk level.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use sqlx::{PgPool, Row as _};
use thiserror::Error;
use uuid::Uuid;

use crate::devices::DeviceStore;
use crate::sentinel::{Attestation, Response, Sentinel, SessionDigest};

/// The persisted state of one device's reading session.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// 64-bit session id; the Ex Libris watermark seed (05 §4).
    pub session_id: u64,
    pub device_id: Uuid,
    pub user_id: Option<Uuid>,
    pub sentinel: Sentinel,
}

/// What an `observe` yields: the session's id and the response to apply now.
#[derive(Debug, Clone, Copy)]
pub struct SessionOutcome {
    pub session_id: u64,
    pub response: Response,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("storage: {0}")]
    Backend(String),
}

impl From<sqlx::Error> for SessionError {
    fn from(e: sqlx::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

fn poisoned<T>(_: T) -> SessionError {
    SessionError::Backend("session lock poisoned".into())
}

/// A fresh, unpredictable 64-bit session id.
fn mint_session_id() -> u64 {
    let mut b = [0u8; 8];
    // A failure to seed would be catastrophic; fall back to a time-based id
    // rather than a predictable constant.
    if aws_lc_rs::rand::fill(&mut b).is_err() {
        return Uuid::now_v7().as_u128() as u64;
    }
    u64::from_be_bytes(b)
}

/// Session registry: in-memory (dev/tests) or Postgres (production).
#[derive(Debug, Clone)]
pub enum SessionStore {
    Memory(Arc<RwLock<HashMap<Uuid, SessionState>>>),
    Postgres(PgPool),
}

impl SessionStore {
    #[must_use]
    pub fn memory() -> Self {
        Self::Memory(Arc::new(RwLock::new(HashMap::new())))
    }

    /// The session store that matches a device store: Postgres shares the pool,
    /// everything else is in-memory.
    #[must_use]
    pub fn alongside(devices: &DeviceStore) -> Self {
        match devices {
            DeviceStore::Postgres(pool) => Self::Postgres(pool.clone()),
            DeviceStore::Memory(_) => Self::memory(),
        }
    }

    /// Gets or creates the device's session, folds `digest` and `attestation`
    /// through its Sentinel, persists the new risk state, and returns the
    /// session id and the response to apply to this lease.
    pub async fn observe(
        &self,
        device_id: Uuid,
        user_id: Option<Uuid>,
        digest: &SessionDigest,
        attestation: Attestation,
    ) -> Result<SessionOutcome, SessionError> {
        match self {
            Self::Memory(m) => {
                let mut map = m.write().map_err(poisoned)?;
                let state = map.entry(device_id).or_insert_with(|| SessionState {
                    session_id: mint_session_id(),
                    device_id,
                    user_id,
                    sentinel: Sentinel::new(),
                });
                if user_id.is_some() {
                    state.user_id = user_id;
                }
                let response = state.sentinel.observe(digest, attestation);
                Ok(SessionOutcome {
                    session_id: state.session_id,
                    response,
                })
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                // Serialize concurrent opens for this device.
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
                    .bind(device_id.to_string())
                    .execute(&mut *tx)
                    .await?;
                let existing = sqlx::query(
                    "SELECT session_id, level, l1_streak FROM sessions WHERE device_id = $1",
                )
                .bind(device_id)
                .fetch_optional(&mut *tx)
                .await?;
                let mut sentinel;
                let session_id: i64;
                if let Some(row) = existing {
                    session_id = row.try_get::<i64, _>("session_id")?;
                    let level: i16 = row.try_get("level")?;
                    let streak: i32 = row.try_get("l1_streak")?;
                    sentinel = Sentinel::restore(level.max(0) as u8, streak.max(0) as u32);
                } else {
                    session_id = mint_session_id() as i64;
                    sentinel = Sentinel::new();
                }
                let response = sentinel.observe(digest, attestation);
                let (level, streak) = sentinel.snapshot();
                sqlx::query(
                    "INSERT INTO sessions (device_id, session_id, user_id, level, l1_streak, last_seen)
                     VALUES ($1, $2, $3, $4, $5, now())
                     ON CONFLICT (device_id) DO UPDATE
                       SET level = $4, l1_streak = $5, user_id = COALESCE($3, sessions.user_id), last_seen = now()",
                )
                .bind(device_id)
                .bind(session_id)
                .bind(user_id)
                .bind(i16::from(level))
                .bind(streak as i32)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(SessionOutcome {
                    session_id: session_id as u64,
                    response,
                })
            }
        }
    }

    /// The current state of a device's session, if any.
    pub async fn get(&self, device_id: Uuid) -> Result<Option<SessionState>, SessionError> {
        match self {
            Self::Memory(m) => Ok(m.read().map_err(poisoned)?.get(&device_id).cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT session_id, user_id, level, l1_streak FROM sessions WHERE device_id = $1",
                )
                .bind(device_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| {
                    let level: i16 = r.try_get("level").unwrap_or(0);
                    let streak: i32 = r.try_get("l1_streak").unwrap_or(0);
                    SessionState {
                        session_id: r.try_get::<i64, _>("session_id").unwrap_or_default() as u64,
                        device_id,
                        user_id: r.try_get("user_id").ok(),
                        sentinel: Sentinel::restore(level.max(0) as u8, streak.max(0) as u32),
                    }
                }))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sentinel::Level;

    fn bot_digest() -> SessionDigest {
        SessionDigest::human(80, 80 * 1000, 0.05) // 1 page/s, metronomic
    }

    #[tokio::test]
    async fn a_new_device_gets_a_stable_session_id_and_starts_normal() {
        let store = SessionStore::memory();
        let dev = Uuid::now_v7();
        let first = store
            .observe(
                dev,
                None,
                &SessionDigest::human(10, 60_000, 0.9),
                Attestation::NotRequested,
            )
            .await
            .unwrap();
        assert_eq!(first.response.level, Level::Normal);
        assert_eq!(first.response.ahead_chunks, crate::sentinel::NORMAL_AHEAD);
        // The same device keeps the same session id across opens.
        let second = store
            .observe(
                dev,
                None,
                &SessionDigest::human(10, 60_000, 0.9),
                Attestation::NotRequested,
            )
            .await
            .unwrap();
        assert_eq!(first.session_id, second.session_id);
        // Two different devices get different session ids.
        let other = store
            .observe(
                Uuid::now_v7(),
                None,
                &SessionDigest::human(5, 60_000, 0.9),
                Attestation::NotRequested,
            )
            .await
            .unwrap();
        assert_ne!(first.session_id, other.session_id);
    }

    #[tokio::test]
    async fn sustained_automation_shrinks_the_window_and_persists_across_opens() {
        let store = SessionStore::memory();
        let dev = Uuid::now_v7();
        let d = bot_digest();
        // First window: watch → prefetch clamped to 1 chunk.
        let w1 = store
            .observe(dev, None, &d, Attestation::NotRequested)
            .await
            .unwrap();
        assert_eq!(w1.response.level, Level::Watch);
        assert_eq!(w1.response.ahead_chunks, 1);
        // The state persisted, so the next open escalates to challenge.
        let w2 = store
            .observe(dev, None, &d, Attestation::NotRequested)
            .await
            .unwrap();
        assert_eq!(w2.response.level, Level::Challenge);
        assert!(w2.response.require_attestation);
        // The id is unchanged throughout.
        assert_eq!(w1.session_id, w2.session_id);
    }

    #[tokio::test]
    async fn clear_automation_pauses_the_session() {
        let store = SessionStore::memory();
        let dev = Uuid::now_v7();
        let d = SessionDigest {
            webdriver: true,
            software_renderer: true,
            datacenter_ip: true,
            visibility_ratio: 0.1,
            ..SessionDigest::human(200, 60_000, 0.02)
        };
        let out = store
            .observe(dev, None, &d, Attestation::NotRequested)
            .await
            .unwrap();
        assert!(out.response.paused, "clear automation pauses");
    }
}
