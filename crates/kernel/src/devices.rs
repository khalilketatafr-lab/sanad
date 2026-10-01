//! Device registry: in-memory (dev, tests) or Postgres (production).
//!
//! A device is a pair of non-extractable WebCrypto keys:
//! - `DeviceKey-Sign` (P-256 ECDSA), identified by its JWK thumbprint
//!   (`dpop_jkt`), which signs every DPoP proof;
//! - `DeviceKey-ECDH` (P-256), which lease keys are wrapped to (stored as an
//!   uncompressed SEC1 point).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use sqlx::PgPool;
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub dpop_jkt: String,
    #[serde(skip)]
    pub ecdh_public: Vec<u8>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct NewDevice {
    pub dpop_jkt: String,
    pub ecdh_public: Vec<u8>,
    pub platform: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("a revoked device cannot be re-registered with the same key")]
    Revoked,
    #[error("this device is signed in to another account")]
    OtherUser,
    #[error("the account already has the maximum number of active devices")]
    DeviceCap,
    #[error("no such device")]
    NotFound,
    #[error("storage: {0}")]
    Backend(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

/// In-memory registry (development and tests). Opaque.
#[derive(Debug, Default)]
pub struct MemoryDevices {
    by_id: HashMap<Uuid, Device>,
    by_jkt: HashMap<String, Uuid>,
}

#[derive(Debug, Clone)]
pub enum DeviceStore {
    Memory(Arc<RwLock<MemoryDevices>>),
    Postgres(PgPool),
}

type Row = (
    Uuid,
    Option<Uuid>,
    String,
    Vec<u8>,
    OffsetDateTime,
    Option<OffsetDateTime>,
);

fn from_row((id, user_id, dpop_jkt, ecdh_public, created_at, revoked_at): Row) -> Device {
    Device {
        id,
        user_id,
        dpop_jkt,
        ecdh_public,
        created_at,
        revoked_at,
    }
}

fn poisoned<T>(_: T) -> StoreError {
    StoreError::Backend("device store lock poisoned".into())
}

impl DeviceStore {
    #[must_use]
    pub fn memory() -> Self {
        Self::Memory(Arc::new(RwLock::new(MemoryDevices::default())))
    }

    /// Registers a device, or returns the existing one for the same signing
    /// key. Re-proving possession of the key is how a device recovers a token.
    /// A revoked key stays revoked.
    pub async fn register(&self, new: NewDevice) -> Result<Device, StoreError> {
        match self {
            Self::Memory(m) => {
                let mut m = m.write().map_err(poisoned)?;
                if let Some(existing) = m.by_jkt.get(&new.dpop_jkt).and_then(|id| m.by_id.get(id)) {
                    return if existing.revoked_at.is_some() {
                        Err(StoreError::Revoked)
                    } else {
                        Ok(existing.clone())
                    };
                }
                let device = Device {
                    id: Uuid::now_v7(),
                    user_id: None,
                    dpop_jkt: new.dpop_jkt.clone(),
                    ecdh_public: new.ecdh_public,
                    created_at: OffsetDateTime::now_utc(),
                    revoked_at: None,
                };
                m.by_jkt.insert(new.dpop_jkt, device.id);
                m.by_id.insert(device.id, device.clone());
                Ok(device)
            }
            Self::Postgres(pool) => {
                // Idempotent on the key; RETURNING yields the row either way.
                // Literal SQL only (sqlx 0.9 rejects dynamically built strings).
                let row: Row = sqlx::query_as(
                    "INSERT INTO devices (id, dpop_jkt, ecdh_pub, platform)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (dpop_jkt) DO UPDATE SET last_seen = now()
                     RETURNING id, user_id, dpop_jkt, ecdh_pub, created_at, revoked_at",
                )
                .bind(Uuid::now_v7())
                .bind(&new.dpop_jkt)
                .bind(&new.ecdh_public)
                .bind(&new.platform)
                .fetch_one(pool)
                .await?;
                let device = from_row(row);
                if device.revoked_at.is_some() {
                    return Err(StoreError::Revoked);
                }
                Ok(device)
            }
        }
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Device>, StoreError> {
        match self {
            Self::Memory(m) => Ok(m.read().map_err(poisoned)?.by_id.get(&id).cloned()),
            Self::Postgres(pool) => {
                let row: Option<Row> = sqlx::query_as(
                    "SELECT id, user_id, dpop_jkt, ecdh_pub, created_at, revoked_at FROM devices WHERE id = $1",
                )
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(from_row))
            }
        }
    }

    /// Attaches a device to a user after a passkey sign-in on it. Idempotent
    /// for the same user. Refused when the device belongs to another account
    /// or the user already has `cap` other active devices.
    pub async fn attach_user(
        &self,
        id: Uuid,
        user_id: Uuid,
        cap: usize,
    ) -> Result<Device, StoreError> {
        match self {
            Self::Memory(m) => {
                let mut m = m.write().map_err(poisoned)?;
                let others = m
                    .by_id
                    .values()
                    .filter(|d| d.id != id && d.user_id == Some(user_id) && d.revoked_at.is_none())
                    .count();
                let device = m.by_id.get_mut(&id).ok_or(StoreError::NotFound)?;
                match device.user_id {
                    Some(u) if u == user_id => return Ok(device.clone()),
                    Some(_) => return Err(StoreError::OtherUser),
                    None if others >= cap => return Err(StoreError::DeviceCap),
                    None => device.user_id = Some(user_id),
                }
                Ok(device.clone())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                // Serialize attaches per user so the cap cannot be raced.
                sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                    .bind(user_id)
                    .fetch_one(&mut *tx)
                    .await?;
                let row: Option<Row> = sqlx::query_as(
                    "SELECT id, user_id, dpop_jkt, ecdh_pub, created_at, revoked_at FROM devices WHERE id = $1 FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
                let device = from_row(row.ok_or(StoreError::NotFound)?);
                match device.user_id {
                    Some(u) if u == user_id => return Ok(device),
                    Some(_) => return Err(StoreError::OtherUser),
                    None => {}
                }
                let (others,): (i64,) = sqlx::query_as(
                    "SELECT count(*) FROM devices WHERE user_id = $1 AND id <> $2 AND revoked_at IS NULL",
                )
                .bind(user_id)
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
                if usize::try_from(others).unwrap_or(usize::MAX) >= cap {
                    return Err(StoreError::DeviceCap);
                }
                sqlx::query("UPDATE devices SET user_id = $2 WHERE id = $1")
                    .bind(id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(Device {
                    user_id: Some(user_id),
                    ..device
                })
            }
        }
    }

    /// Revokes a device: its tokens and proofs stop working immediately.
    pub async fn revoke(&self, id: Uuid) -> Result<bool, StoreError> {
        match self {
            Self::Memory(m) => {
                let mut m = m.write().map_err(poisoned)?;
                Ok(m.by_id.get_mut(&id).is_some_and(|d| {
                    d.revoked_at.get_or_insert_with(OffsetDateTime::now_utc);
                    true
                }))
            }
            Self::Postgres(pool) => {
                let done = sqlx::query(
                    "UPDATE devices SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
                )
                .bind(id)
                .execute(pool)
                .await?;
                Ok(done.rows_affected() == 1)
            }
        }
    }
}
