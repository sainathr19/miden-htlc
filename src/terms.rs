//! The terms of an HTLC and their layout in the note storage.

use miden_client::Felt;
use miden_client::account::AccountId;
use miden_client::block::BlockNumber;

use crate::secret::NUM_LIMBS;
use crate::{Hashlock, HtlcError};

/// The note storage layout. It must match the constants in `masm/htlc.masm`.
pub(crate) mod layout {
    use core::ops::Range;

    pub const NUM_STORAGE_ITEMS: usize = 13;
    pub const HASHLOCK: Range<usize> = 0..8;
    pub const RECIPIENT_SUFFIX: usize = 8;
    pub const RECIPIENT_PREFIX: usize = 9;
    pub const REFUND_SUFFIX: usize = 10;
    pub const REFUND_PREFIX: usize = 11;
    pub const EXPIRY: usize = 12;

    // The hashlock is exactly one SHA-256 digest of u32 limbs.
    const _: () = assert!(HASHLOCK.end - HASHLOCK.start == super::NUM_LIMBS);
}

/// The terms of an HTLC. They are stored in the note and enforced by the note script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtlcTerms {
    /// SHA-256 of the secret. A claim must supply the secret.
    pub hashlock: Hashlock,
    /// The account that a claim pays.
    pub recipient: AccountId,
    /// The account that a refund pays.
    pub refund: AccountId,
    /// The first block at which a refund is possible.
    pub expiry: BlockNumber,
}

impl HtlcTerms {
    /// Returns `true` if a refund is possible at `block`.
    #[must_use]
    pub fn is_expired_at(&self, block: BlockNumber) -> bool {
        block >= self.expiry
    }

    /// The note storage items, in the order that the note script reads them.
    #[must_use]
    pub fn to_storage_items(&self) -> Vec<Felt> {
        let mut items = Vec::with_capacity(layout::NUM_STORAGE_ITEMS);
        items.extend(self.hashlock.to_limbs());
        items.push(self.recipient.suffix());
        items.push(self.recipient.prefix().as_felt());
        items.push(self.refund.suffix());
        items.push(self.refund.prefix().as_felt());
        items.push(Felt::from(self.expiry.as_u32()));
        items
    }

    /// Reads terms from note storage items.
    ///
    /// # Errors
    ///
    /// Returns an error if there are not exactly 13 items, if a hashlock limb or the expiry is not
    /// a u32, or if an account ID is invalid.
    pub fn from_storage_items(items: &[Felt]) -> Result<Self, HtlcError> {
        if items.len() != layout::NUM_STORAGE_ITEMS {
            return Err(HtlcError::StorageLength {
                expected: layout::NUM_STORAGE_ITEMS,
                found: items.len(),
            });
        }
        let expiry = u32::try_from(items[layout::EXPIRY].as_canonical_u64()).map_err(|_| {
            HtlcError::NotU32 {
                index: layout::EXPIRY,
            }
        })?;
        Ok(Self {
            hashlock: Hashlock::from_limbs(&items[layout::HASHLOCK])?,
            recipient: AccountId::try_from_elements(
                items[layout::RECIPIENT_SUFFIX],
                items[layout::RECIPIENT_PREFIX],
            )?,
            refund: AccountId::try_from_elements(
                items[layout::REFUND_SUFFIX],
                items[layout::REFUND_PREFIX],
            )?,
            expiry: BlockNumber::from(expiry),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HTLC_MASM;

    #[test]
    fn layout_matches_the_script() {
        let expected = [
            format!("const NUM_STORAGE_ITEMS={}", layout::NUM_STORAGE_ITEMS),
            format!("const HASHLOCK_LO_PTR={}", layout::HASHLOCK.start),
            format!("const HASHLOCK_HI_PTR={}", layout::HASHLOCK.start + 4),
            format!("const RECIPIENT_SUFFIX_PTR={}", layout::RECIPIENT_SUFFIX),
            format!("const RECIPIENT_PREFIX_PTR={}", layout::RECIPIENT_PREFIX),
            format!("const REFUND_SUFFIX_PTR={}", layout::REFUND_SUFFIX),
            format!("const REFUND_PREFIX_PTR={}", layout::REFUND_PREFIX),
            format!("const EXPIRY_PTR={}", layout::EXPIRY),
        ];
        for line in expected {
            assert!(
                HTLC_MASM.contains(&line),
                "htlc.masm does not contain `{line}`"
            );
        }
    }

    #[test]
    fn rejects_wrong_storage_length() {
        let err = HtlcTerms::from_storage_items(&[Felt::from(0u32); 4]).unwrap_err();
        assert!(matches!(
            err,
            HtlcError::StorageLength {
                expected: 13,
                found: 4
            }
        ));
    }
}
