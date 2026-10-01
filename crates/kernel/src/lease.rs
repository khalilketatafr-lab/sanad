//! Lease key wrapping (blueprint 01 §3.3).
//!
//! ```text
//! EK  = fresh ECDH P-256 key pair, one per lease
//! Z   = ECDH(EK.private, DeviceKey-ECDH.public)
//! KEK = HKDF-SHA256(Z, salt = lease_id (16 bytes), info = "sanad/lease/v1")
//! wrapped[i] = AES-KW(KEK, CK[i])          RFC 3394, 40 bytes per 256-bit key
//! ```
//!
//! Only `EK.public` and the wrapped keys leave the Kernel. The device unwraps
//! with WebCrypto (`deriveBits` → HKDF `deriveKey` → `unwrapKey`) into
//! non-extractable AES-GCM keys (`packages/lumen/src/vault/lease.ts`). `EK`'s
//! private half and the KEK exist only inside [`wrap_for_device`], and the KEK
//! is zeroized on drop. A leaked KEK opens one window for one device (P5).

use aws_lc_rs::agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral};
use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use aws_lc_rs::key_wrap::{AES_256, AesKek, KeyWrap};
use aws_lc_rs::rand::SystemRandom;
use sanad_folio::seal::{ChunkKey, KEY_LEN};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const LEASE_INFO: &[u8] = b"sanad/lease/v1";
/// RFC 3394 output for a 256-bit key: 64-bit integrity block + 256 bits.
pub const WRAPPED_KEY_LEN: usize = KEY_LEN + 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LeaseError {
    #[error("device ECDH key is not a valid P-256 point")]
    DevicePoint,
    #[error("key agreement or derivation failed")]
    Crypto,
}

/// One chunk key, wrapped to the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    pub chunk: u32,
    pub variant: u8,
    pub wrapped: [u8; WRAPPED_KEY_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLease {
    /// Ephemeral public key, uncompressed SEC1 (65 bytes).
    pub ephemeral_public: Vec<u8>,
    pub keys: Vec<WrappedKey>,
}

/// The lease KEK from a shared secret: HKDF-SHA256(Z, lease_id, LEASE_INFO).
fn lease_kek(z: &[u8], lease_id: &Uuid) -> Result<Zeroizing<[u8; KEY_LEN]>, LeaseError> {
    let prk = Salt::new(HKDF_SHA256, lease_id.as_bytes()).extract(z);
    let okm = prk
        .expand(&[LEASE_INFO], HKDF_SHA256)
        .map_err(|_| LeaseError::Crypto)?;
    let mut kek = Zeroizing::new([0u8; KEY_LEN]);
    okm.fill(kek.as_mut()).map_err(|_| LeaseError::Crypto)?;
    Ok(kek)
}

/// Wraps `keys` (chunk index, variant, key) to the device's ECDH public key
/// under a fresh ephemeral key.
pub fn wrap_for_device(
    device_ecdh_public: &[u8],
    lease_id: &Uuid,
    keys: &[(u32, u8, &ChunkKey)],
) -> Result<WrappedLease, LeaseError> {
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &SystemRandom::new())
        .map_err(|_| LeaseError::Crypto)?;
    let ephemeral_public = ephemeral
        .compute_public_key()
        .map_err(|_| LeaseError::Crypto)?
        .as_ref()
        .to_vec();
    let peer = UnparsedPublicKey::new(&ECDH_P256, device_ecdh_public);
    let kek = agree_ephemeral(ephemeral, peer, LeaseError::DevicePoint, |z| {
        lease_kek(z, lease_id)
    })?;
    let keys = keys
        .iter()
        .map(|&(chunk, variant, ck)| {
            let mut wrapped = [0u8; WRAPPED_KEY_LEN];
            AesKek::new(&AES_256, kek.as_ref())
                .and_then(|k| k.wrap(ck.expose_for_wrapping(), &mut wrapped).map(|_| ()))
                .map_err(|_| LeaseError::Crypto)?;
            Ok(WrappedKey {
                chunk,
                variant,
                wrapped,
            })
        })
        .collect::<Result<Vec<_>, LeaseError>>()?;
    Ok(WrappedLease {
        ephemeral_public,
        keys,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use aws_lc_rs::agreement::{PrivateKey, agree};
    use sanad_folio::seal::{TitleMasterKey, derive_chunk_key};

    /// What the device does (WebCrypto in production): ECDH with its private
    /// key, HKDF, AES-KW unwrap.
    fn device_unwrap(
        device: &PrivateKey,
        lease_id: &Uuid,
        lease: &WrappedLease,
        k: &WrappedKey,
    ) -> Vec<u8> {
        let peer = UnparsedPublicKey::new(&ECDH_P256, lease.ephemeral_public.as_slice());
        let kek = agree(device, peer, LeaseError::Crypto, |z| lease_kek(z, lease_id)).unwrap();
        let mut out = [0u8; KEY_LEN];
        AesKek::new(&AES_256, kek.as_ref())
            .unwrap()
            .unwrap(&k.wrapped, &mut out)
            .unwrap()
            .to_vec()
    }

    fn setup() -> (PrivateKey, Vec<u8>, Vec<ChunkKey>) {
        let device = PrivateKey::generate(&ECDH_P256).unwrap();
        let public = device.compute_public_key().unwrap().as_ref().to_vec();
        let tmk = TitleMasterKey::from_bytes([0x42; 32]);
        let cks = (0..3)
            .map(|i| derive_chunk_key(&tmk, &[7; 16], 0, i).unwrap())
            .collect();
        (device, public, cks)
    }

    #[test]
    fn device_recovers_exactly_the_chunk_keys() {
        let (device, public, cks) = setup();
        let lease_id = Uuid::now_v7();
        let input: Vec<(u32, u8, &ChunkKey)> = cks
            .iter()
            .enumerate()
            .map(|(i, k)| (i as u32, 0, k))
            .collect();
        let lease = wrap_for_device(&public, &lease_id, &input).unwrap();
        assert_eq!(lease.ephemeral_public.len(), 65);
        assert_eq!(lease.keys.len(), 3);
        for (k, ck) in lease.keys.iter().zip(&cks) {
            assert_eq!(
                device_unwrap(&device, &lease_id, &lease, k),
                ck.expose_for_wrapping()
            );
            assert_ne!(
                &k.wrapped[8..],
                ck.expose_for_wrapping(),
                "never the raw key"
            );
        }
    }

    /// Unwrap attempt with an arbitrary device key and lease id.
    fn try_unwrap(device: &PrivateKey, lease_id: &Uuid, lease: &WrappedLease) -> bool {
        let peer = UnparsedPublicKey::new(&ECDH_P256, lease.ephemeral_public.as_slice());
        let kek = agree(device, peer, LeaseError::Crypto, |z| lease_kek(z, lease_id)).unwrap();
        let mut out = [0u8; KEY_LEN];
        AesKek::new(&AES_256, kek.as_ref())
            .unwrap()
            .unwrap(&lease.keys[0].wrapped, &mut out)
            .is_ok()
    }

    #[test]
    fn only_the_device_with_the_right_lease_id_can_unwrap() {
        let (device, public, cks) = setup();
        let lease_id = Uuid::now_v7();
        let lease = wrap_for_device(&public, &lease_id, &[(0, 0, &cks[0])]).unwrap();
        assert!(try_unwrap(&device, &lease_id, &lease));
        // AES-KW's integrity block rejects any other KEK.
        let other = PrivateKey::generate(&ECDH_P256).unwrap();
        assert!(!try_unwrap(&other, &lease_id, &lease), "another device");
        assert!(
            !try_unwrap(&device, &Uuid::now_v7(), &lease),
            "another lease id"
        );
    }

    #[test]
    fn every_lease_uses_a_fresh_ephemeral_key() {
        let (_, public, cks) = setup();
        let id = Uuid::now_v7();
        let a = wrap_for_device(&public, &id, &[(0, 0, &cks[0])]).unwrap();
        let b = wrap_for_device(&public, &id, &[(0, 0, &cks[0])]).unwrap();
        assert_ne!(a.ephemeral_public, b.ephemeral_public);
        assert_ne!(a.keys[0].wrapped, b.keys[0].wrapped);
    }

    #[test]
    fn rejects_an_invalid_device_point() {
        let (_, _, cks) = setup();
        let mut bad = vec![4u8; 65];
        bad[1] = 1;
        assert_eq!(
            wrap_for_device(&bad, &Uuid::now_v7(), &[(0, 0, &cks[0])]),
            Err(LeaseError::DevicePoint)
        );
    }
}
