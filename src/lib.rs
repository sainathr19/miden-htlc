//! A hash time-locked contract (HTLC) note for the Miden rollup.
//!
//! An HTLC note holds one asset. Anyone who knows the secret ([`Preimage`]) can claim it: the note
//! script pays the recipient and publishes the secret. From the expiry block on, anyone can refund
//! it to the refund account.
//!
//! ```no_run
//! # use miden_htlc::{HtlcNote, HtlcTerms, Preimage};
//! # fn example(
//! #     alice: miden_client::account::AccountId,
//! #     bob: miden_client::account::AccountId,
//! #     expiry: miden_client::block::BlockNumber,
//! #     asset: miden_client::asset::Asset,
//! #     serial: miden_client::Word,
//! # ) -> Result<(), miden_htlc::HtlcError> {
//! let secret = Preimage::new([7; 32]);
//! let terms = HtlcTerms { hashlock: secret.hashlock(), recipient: bob, refund: alice, expiry };
//! let htlc = HtlcNote::new(alice, terms, asset, serial)?;
//!
//! let lock = htlc.lock_request()?; // alice's transaction
//! let claim = htlc.claim_request(&secret)?; // anyone's transaction
//! let refund = htlc.refund_request()?; // anyone's transaction, from the expiry block on
//! # Ok(())
//! # }
//! ```

mod error;
mod note;
mod script;
mod secret;
mod terms;

pub use error::HtlcError;
pub use note::{HtlcNote, Payout, revealed_preimage};
pub use script::{HTLC_MASM, HtlcScriptError, PREIMAGE_ATTACHMENT_SCHEME, htlc_script};
pub use secret::{Hashlock, Preimage};
pub use terms::HtlcTerms;
