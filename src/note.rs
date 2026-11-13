//! The HTLC note, and the transactions that lock, claim and refund it.

use miden_client::account::AccountId;
use miden_client::asset::Asset;
use miden_client::note::{
    Note, NoteAssets, NoteAttachmentScheme, NoteAttachments, NoteId, NoteRecipient, NoteStorage,
    NoteTag, NoteType, P2idNoteStorage, PartialNoteMetadata,
};
use miden_client::transaction::{TransactionRequest, TransactionRequestBuilder};
use miden_client::{Felt, Word};

use crate::{HtlcError, HtlcTerms, PREIMAGE_ATTACHMENT_SCHEME, Preimage, htlc_script};

/// Which path of the note script created a payout note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payout {
    /// A claim with the preimage. Pays the recipient.
    Claim,
    /// A refund after expiry. Pays the refund account.
    Refund,
}

impl Payout {
    /// The value the note script adds to element 0 of the HTLC serial number to get the payout
    /// note's serial number. Must match `CLAIM_SERIAL_OFFSET` and `REFUND_SERIAL_OFFSET` in
    /// `masm/htlc.masm`.
    #[must_use]
    pub const fn serial_offset(self) -> u32 {
        match self {
            Self::Claim => 1,
            Self::Refund => 2,
        }
    }
}

/// A public note that holds one asset under HTLC terms.
///
/// An `HtlcNote` always runs the HTLC script and has valid terms: [`HtlcNote::new`] builds one, and
/// [`HtlcNote::from_note`] checks a note received from someone else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtlcNote {
    note: Note,
    terms: HtlcTerms,
}

impl HtlcNote {
    /// Builds a public HTLC note that holds `asset`.
    ///
    /// The note carries the recipient's tag, so the recipient's client finds it during sync.
    ///
    /// # Errors
    ///
    /// Returns an error if the note cannot be built, for example if the asset is invalid.
    pub fn new(
        sender: AccountId,
        terms: HtlcTerms,
        asset: Asset,
        serial_num: Word,
    ) -> Result<Self, HtlcError> {
        let storage = NoteStorage::new(terms.to_storage_items())?;
        let recipient = NoteRecipient::new(serial_num, htlc_script().clone(), storage);
        let metadata = PartialNoteMetadata::new(sender, NoteType::Public)
            .with_tag(NoteTag::with_account_target(terms.recipient));
        let assets = NoteAssets::new(vec![asset])?;
        Ok(Self {
            note: Note::new(assets, metadata, recipient),
            terms,
        })
    }

    /// Checks that `note` is an HTLC note and reads its terms.
    ///
    /// Use this before acting on a note that someone else created.
    ///
    /// # Errors
    ///
    /// Returns an error if the note does not run the HTLC script, does not hold exactly one asset,
    /// or has storage that is not valid HTLC terms.
    pub fn from_note(note: Note) -> Result<Self, HtlcError> {
        if note.recipient().script().root() != htlc_script().root() {
            return Err(HtlcError::NotHtlcScript);
        }
        let num_assets = note.assets().num_assets();
        if num_assets != 1 {
            return Err(HtlcError::AssetCount(num_assets));
        }
        let terms = HtlcTerms::from_storage_items(note.recipient().storage().items())?;
        Ok(Self { note, terms })
    }

    /// The note's terms.
    #[must_use]
    pub const fn terms(&self) -> &HtlcTerms {
        &self.terms
    }

    /// The underlying note.
    #[must_use]
    pub const fn note(&self) -> &Note {
        &self.note
    }

    /// The note's ID.
    #[must_use]
    pub fn id(&self) -> NoteId {
        self.note.id()
    }

    /// The asset the note holds.
    ///
    /// # Panics
    ///
    /// Never in practice: an `HtlcNote` always holds exactly one asset.
    #[must_use]
    pub fn asset(&self) -> Asset {
        *self
            .note
            .assets()
            .iter()
            .next()
            .expect("an HTLC note holds exactly one asset")
    }

    /// The sender's transaction: moves the asset from the sender's vault into this note.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction request cannot be built.
    pub fn lock_request(&self) -> Result<TransactionRequest, HtlcError> {
        Ok(TransactionRequestBuilder::new()
            .own_output_notes([self.note.clone()])
            .build()?)
    }

    /// Anyone's transaction: claims the note with the preimage. The note script pays the
    /// recipient and publishes the preimage.
    ///
    /// # Errors
    ///
    /// Returns [`HtlcError::PreimageMismatch`] if the preimage does not unlock the hashlock, so a
    /// doomed transaction is never built. Returns an error if the request cannot be built.
    pub fn claim_request(&self, preimage: &Preimage) -> Result<TransactionRequest, HtlcError> {
        if !self.terms.hashlock.is_unlocked_by(preimage) {
            return Err(HtlcError::PreimageMismatch);
        }
        self.claim_request_unchecked(preimage)
    }

    /// Like [`HtlcNote::claim_request`], but without the local preimage check.
    ///
    /// The note script still rejects a wrong preimage. Use this only to show that it does.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be built.
    pub fn claim_request_unchecked(
        &self,
        preimage: &Preimage,
    ) -> Result<TransactionRequest, HtlcError> {
        let (key, value) = preimage.claim_advice();
        Ok(TransactionRequestBuilder::new()
            .input_notes([(self.note.clone(), Some(key))])
            .extend_advice_map([(key, value)])
            .build()?)
    }

    /// Anyone's transaction: refunds the note to the refund account. The note script rejects it
    /// before the expiry block.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be built.
    pub fn refund_request(&self) -> Result<TransactionRequest, HtlcError> {
        Ok(TransactionRequestBuilder::new()
            .input_notes([(self.note.clone(), None)])
            .build()?)
    }

    /// The recipient of the payout note that a claim or a refund creates. Compare its digest with
    /// an output note's recipient digest to recognise the payout note.
    #[must_use]
    pub fn payout_recipient(&self, payout: Payout) -> NoteRecipient {
        let target = match payout {
            Payout::Claim => self.terms.recipient,
            Payout::Refund => self.terms.refund,
        };
        let serial = self.note.recipient().serial_num();
        let payout_serial = Word::from([
            serial[0] + Felt::from(payout.serial_offset()),
            serial[1],
            serial[2],
            serial[3],
        ]);
        P2idNoteStorage::new(target).into_recipient(payout_serial)
    }
}

/// Reads the preimage that a claim published on its payout note, if the note carries one.
///
/// # Errors
///
/// Returns an error if the attachment exists but does not hold a valid preimage.
pub fn revealed_preimage(attachments: &NoteAttachments) -> Result<Option<Preimage>, HtlcError> {
    let scheme = NoteAttachmentScheme::new(PREIMAGE_ATTACHMENT_SCHEME)?;
    attachments
        .find(scheme)
        .map(|attachment| Preimage::from_limbs(attachment.content().as_elements()))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HTLC_MASM;

    #[test]
    fn serial_offsets_match_the_script() {
        let claim = format!(
            "const CLAIM_SERIAL_OFFSET={}",
            Payout::Claim.serial_offset()
        );
        let refund = format!(
            "const REFUND_SERIAL_OFFSET={}",
            Payout::Refund.serial_offset()
        );
        assert!(HTLC_MASM.contains(&claim));
        assert!(HTLC_MASM.contains(&refund));
    }
}
