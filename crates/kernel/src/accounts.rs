//! Passkey accounts: users and their WebAuthn credentials.
//!
//! A user is a random id plus an opaque WebAuthn user handle; there is no
//! password, no email requirement, nothing to stuff or phish. Devices are
//! attached to users by [`crate::devices::DeviceStore::attach_user`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::devices::DeviceStore;
use crate::webauthn::NewCredential;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPasskey {
    pub credential_id: Vec<u8>,
    pub user_id: Uuid,
    pub user_handle: Vec<u8>,
    pub public_key_cose: Vec<u8>,
    pub sign_count: u32,
}

#[derive(Debug, Error)]
pub enum AccountError {
    #[error("this passkey is already registered")]
    CredentialExists,
    #[error("storage: {0}")]
    Backend(String),
}

impl From<sqlx::Error> for AccountError {
    fn from(e: sqlx::Error) -> Self {
        match e
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
        {
            Some(code) if code == "23505" => Self::CredentialExists, // unique_violation
            _ => Self::Backend(e.to_string()),
        }
    }
}

/// In-memory accounts (development and tests). Opaque.
#[derive(Debug, Default)]
pub struct MemoryAccounts {
    users: HashMap<Uuid, Vec<u8>>,
    passkeys: HashMap<Vec<u8>, StoredPasskey>,
}

#[derive(Debug, Clone)]
pub enum AccountStore {
    Memory(Arc<RwLock<MemoryAccounts>>),
    Postgres(PgPool),
}

fn poisoned<T>(_: T) -> AccountError {
    AccountError::Backend("account store lock poisoned".into())
}

type PasskeyRow = (Vec<u8>, Uuid, Vec<u8>, Vec<u8>, i64);

impl AccountStore {
    /// The account store sharing `devices`' backend.
    #[must_use]
    pub fn alongside(devices: &DeviceStore) -> Self {
        match devices {
            DeviceStore::Memory(_) => Self::Memory(Arc::default()),
            DeviceStore::Postgres(pool) => Self::Postgres(pool.clone()),
        }
    }

    /// Creates a user whose first credential is `cred`.
    pub async fn create_user_with_passkey(
        &self,
        user_id: Uuid,
        user_handle: &[u8],
        cred: &NewCredential,
    ) -> Result<(), AccountError> {
        match self {
            Self::Memory(m) => {
                let mut m = m.write().map_err(poisoned)?;
                if m.passkeys.contains_key(&cred.credential_id) {
                    return Err(AccountError::CredentialExists);
                }
                m.users.insert(user_id, user_handle.to_vec());
                m.passkeys.insert(
                    cred.credential_id.clone(),
                    StoredPasskey {
                        credential_id: cred.credential_id.clone(),
                        user_id,
                        user_handle: user_handle.to_vec(),
                        public_key_cose: cred.public_key_cose.clone(),
                        sign_count: cred.sign_count,
                    },
                );
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("INSERT INTO users (id, user_handle) VALUES ($1, $2)")
                    .bind(user_id)
                    .bind(user_handle)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO passkeys (credential_id, user_id, public_key_cose, alg, sign_count, aaguid, backup_eligible, backed_up)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(&cred.credential_id)
                .bind(user_id)
                .bind(&cred.public_key_cose)
                .bind(i16::try_from(cred.alg).map_err(|_| AccountError::Backend("alg".into()))?)
                .bind(i64::from(cred.sign_count))
                .bind(cred.aaguid.as_slice())
                .bind(cred.backup_eligible)
                .bind(cred.backed_up)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    /// Adds another passkey to an existing user.
    pub async fn add_passkey(
        &self,
        user_id: Uuid,
        cred: &NewCredential,
    ) -> Result<(), AccountError> {
        match self {
            Self::Memory(m) => {
                let mut m = m.write().map_err(poisoned)?;
                let handle = m
                    .users
                    .get(&user_id)
                    .cloned()
                    .ok_or_else(|| AccountError::Backend("no such user".into()))?;
                if m.passkeys.contains_key(&cred.credential_id) {
                    return Err(AccountError::CredentialExists);
                }
                m.passkeys.insert(
                    cred.credential_id.clone(),
                    StoredPasskey {
                        credential_id: cred.credential_id.clone(),
                        user_id,
                        user_handle: handle,
                        public_key_cose: cred.public_key_cose.clone(),
                        sign_count: cred.sign_count,
                    },
                );
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO passkeys (credential_id, user_id, public_key_cose, alg, sign_count, aaguid, backup_eligible, backed_up)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(&cred.credential_id)
                .bind(user_id)
                .bind(&cred.public_key_cose)
                .bind(i16::try_from(cred.alg).map_err(|_| AccountError::Backend("alg".into()))?)
                .bind(i64::from(cred.sign_count))
                .bind(cred.aaguid.as_slice())
                .bind(cred.backup_eligible)
                .bind(cred.backed_up)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    /// The user handle of an existing user.
    pub async fn user_handle(&self, user_id: Uuid) -> Result<Option<Vec<u8>>, AccountError> {
        match self {
            Self::Memory(m) => Ok(m.read().map_err(poisoned)?.users.get(&user_id).cloned()),
            Self::Postgres(pool) => {
                let row: Option<(Vec<u8>,)> =
                    sqlx::query_as("SELECT user_handle FROM users WHERE id = $1")
                        .bind(user_id)
                        .fetch_optional(pool)
                        .await?;
                Ok(row.map(|(h,)| h))
            }
        }
    }

    pub async fn passkey(
        &self,
        credential_id: &[u8],
    ) -> Result<Option<StoredPasskey>, AccountError> {
        match self {
            Self::Memory(m) => Ok(m
                .read()
                .map_err(poisoned)?
                .passkeys
                .get(credential_id)
                .cloned()),
            Self::Postgres(pool) => {
                let row: Option<PasskeyRow> = sqlx::query_as(
                    "SELECT p.credential_id, p.user_id, u.user_handle, p.public_key_cose, p.sign_count
                     FROM passkeys p JOIN users u ON u.id = p.user_id WHERE p.credential_id = $1",
                )
                .bind(credential_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(
                    |(credential_id, user_id, user_handle, public_key_cose, count)| StoredPasskey {
                        credential_id,
                        user_id,
                        user_handle,
                        public_key_cose,
                        sign_count: u32::try_from(count).unwrap_or(u32::MAX),
                    },
                ))
            }
        }
    }

    /// Records a successful assertion: new counter and backup state.
    pub async fn record_use(
        &self,
        credential_id: &[u8],
        sign_count: u32,
        backed_up: bool,
    ) -> Result<(), AccountError> {
        match self {
            Self::Memory(m) => {
                if let Some(p) = m.write().map_err(poisoned)?.passkeys.get_mut(credential_id) {
                    p.sign_count = sign_count;
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    "UPDATE passkeys SET sign_count = $2, backed_up = $3 AND backup_eligible, last_used_at = now()
                     WHERE credential_id = $1",
                )
                .bind(credential_id)
                .bind(i64::from(sign_count))
                .bind(backed_up)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}
