//! The secret (preimage) and its SHA-256 hash (hashlock).
//!
//! Both are 32 bytes. The note script handles them as 8 big-endian u32 limbs, because that is the
//! input and output format of the core library's `sha256::hash` procedure.

use core::fmt;

use miden_client::{Felt, Word};
use sha2::{Digest, Sha256};

use crate::HtlcError;

/// The number of u32 limbs in 32 bytes.
pub(crate) const NUM_LIMBS: usize = 8;

/// The first element of the advice map key under which a claim supplies the preimage.
///
/// The key must not be the empty word, because empty note args select the refund path.
const CLAIM_KEY_TAG: u32 = 0x4854_4c43; // "HTLC"

/// The secret that unlocks an HTLC.
///
/// The `Debug` output never shows the bytes, so a preimage cannot leak into logs by accident.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Preimage([u8; 32]);

impl Preimage {
    /// Wraps 32 secret bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the secret bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the hashlock that this preimage unlocks: its SHA-256 digest.
    #[must_use]
    pub fn hashlock(&self) -> Hashlock {
        Hashlock(Sha256::digest(self.0).into())
    }

    /// The inputs a claim gives the note script: the note args, and the advice map entry that the
    /// note args point to. The entry's value is the preimage as 8 big-endian u32 limbs.
    ///
    /// [`HtlcNote::claim_request`](crate::HtlcNote::claim_request) uses this. Call it directly
    /// only when you build the transaction another way, for example in tests.
    #[must_use]
    pub fn claim_advice(&self) -> (Word, Vec<Felt>) {
        let limbs = self.to_limbs();
        let key = Word::from([Felt::from(CLAIM_KEY_TAG), limbs[0], limbs[1], limbs[2]]);
        (key, limbs.to_vec())
    }

    pub(crate) fn to_limbs(self) -> [Felt; NUM_LIMBS] {
        bytes_to_limbs(&self.0)
    }

    pub(crate) fn from_limbs(limbs: &[Felt]) -> Result<Self, HtlcError> {
        limbs_to_bytes(limbs).map(Self)
    }
}

impl From<[u8; 32]> for Preimage {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

impl fmt::Debug for Preimage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Preimage(<redacted>)")
    }
}

/// The SHA-256 hash of a preimage. Public: it goes in the note storage.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hashlock([u8; 32]);

impl Hashlock {
    /// Wraps a 32-byte SHA-256 digest, for example one taken from the other chain of a swap.
    #[must_use]
    pub const fn new(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns `true` if `preimage` hashes to this hashlock.
    #[must_use]
    pub fn is_unlocked_by(&self, preimage: &Preimage) -> bool {
        preimage.hashlock() == *self
    }

    pub(crate) fn to_limbs(self) -> [Felt; NUM_LIMBS] {
        bytes_to_limbs(&self.0)
    }

    pub(crate) fn from_limbs(limbs: &[Felt]) -> Result<Self, HtlcError> {
        limbs_to_bytes(limbs).map(Self)
    }
}

impl fmt::Display for Hashlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Hashlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hashlock({self})")
    }
}

fn bytes_to_limbs(bytes: &[u8; 32]) -> [Felt; NUM_LIMBS] {
    core::array::from_fn(|i| {
        let mut chunk = [0u8; 4];
        chunk.copy_from_slice(&bytes[4 * i..4 * i + 4]);
        Felt::from(u32::from_be_bytes(chunk))
    })
}

fn limbs_to_bytes(limbs: &[Felt]) -> Result<[u8; 32], HtlcError> {
    if limbs.len() != NUM_LIMBS {
        return Err(HtlcError::StorageLength {
            expected: NUM_LIMBS,
            found: limbs.len(),
        });
    }
    let mut bytes = [0u8; 32];
    for (index, limb) in limbs.iter().enumerate() {
        let value =
            u32::try_from(limb.as_canonical_u64()).map_err(|_| HtlcError::NotU32 { index })?;
        bytes[4 * index..4 * index + 4].copy_from_slice(&value.to_be_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limbs_round_trip() {
        let bytes: [u8; 32] =
            core::array::from_fn(|i| u8::try_from(i * 7 + 200).unwrap_or(u8::MAX));
        let preimage = Preimage::new(bytes);
        assert_eq!(
            Preimage::from_limbs(&preimage.to_limbs()).unwrap(),
            preimage
        );
    }

    #[test]
    fn hashlock_is_sha256() {
        // SHA-256 of 32 zero bytes.
        let expected = "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925";
        assert_eq!(Preimage::new([0; 32]).hashlock().to_string(), expected);
    }

    #[test]
    fn claim_key_is_never_empty() {
        let (key, value) = Preimage::new([0; 32]).claim_advice();
        assert_ne!(key, Word::empty());
        assert_eq!(value.len(), NUM_LIMBS);
    }

    #[test]
    fn debug_hides_the_preimage() {
        assert_eq!(
            format!("{:?}", Preimage::new([7; 32])),
            "Preimage(<redacted>)"
        );
    }

    #[test]
    fn limbs_reject_non_u32_values() {
        let mut limbs = [Felt::from(1u32); NUM_LIMBS];
        limbs[3] = Felt::new(u64::from(u32::MAX) + 1).expect("2^32 is a valid field element");
        assert!(matches!(
            Hashlock::from_limbs(&limbs),
            Err(HtlcError::NotU32 { index: 3 })
        ));
    }
}
