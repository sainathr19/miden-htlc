//! The library's error type.

use miden_client::account::AccountIdError;
use miden_client::note::NoteError;
use miden_client::transaction::TransactionRequestError;

/// An error from building, parsing or using an HTLC note.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HtlcError {
    /// The note's script is not the HTLC script.
    #[error("the note is not an HTLC note: its script root differs from the HTLC script root")]
    NotHtlcScript,

    /// The note's storage does not have the HTLC layout.
    #[error("HTLC note storage must have {expected} items, found {found}")]
    StorageLength {
        /// The number of items an HTLC note stores.
        expected: usize,
        /// The number of items found.
        found: usize,
    },

    /// A value in the note storage that must be a u32 is not one.
    #[error("HTLC note storage item {index} must be a u32 value")]
    NotU32 {
        /// The position of the item in the note storage.
        index: usize,
    },

    /// The note does not hold exactly one asset.
    #[error("an HTLC note must hold exactly one asset, found {0}")]
    AssetCount(usize),

    /// The preimage does not hash to the hashlock.
    #[error("the preimage does not hash to the hashlock")]
    PreimageMismatch,

    /// An account ID in the note storage is invalid.
    #[error("invalid account ID in the HTLC note storage")]
    AccountId(#[from] AccountIdError),

    /// The note could not be built.
    #[error("failed to build the note")]
    Note(#[from] NoteError),

    /// The transaction request could not be built.
    #[error("failed to build the transaction request")]
    Request(#[from] TransactionRequestError),
}
