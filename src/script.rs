//! The HTLC note script and the errors it can raise.

use core::fmt;
use std::sync::LazyLock;

use miden_client::assembly::CodeBuilder;
use miden_client::note::NoteScript;
use miden_client::vm::error_code_from_msg;

/// The MASM source of the HTLC note script.
pub const HTLC_MASM: &str = include_str!("../masm/htlc.masm");

/// The attachment scheme that marks the revealed preimage on a claim's payout note.
pub const PREIMAGE_ATTACHMENT_SCHEME: u16 = 1000;

static HTLC_SCRIPT: LazyLock<NoteScript> = LazyLock::new(|| {
    CodeBuilder::new()
        .compile_note_script(HTLC_MASM)
        .expect("the embedded HTLC script compiles; the `script_compiles` test checks this")
});

/// Returns the compiled HTLC note script. It is compiled once, on first use.
#[must_use]
pub fn htlc_script() -> &'static NoteScript {
    &HTLC_SCRIPT
}

/// A failed assertion in the HTLC note script.
///
/// A note script that the node returns does not carry its error messages, so a failure reports
/// only a numeric code. Each code is a hash of its message, so [`HtlcScriptError::from_error_code`]
/// maps a code back to its variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HtlcScriptError {
    /// The note storage does not have exactly 13 items.
    WrongNumberOfStorageItems,
    /// The claim's preimage does not hash to the hashlock.
    WrongPreimage,
    /// A refund was attempted before the expiry block.
    NotExpired,
    /// The note does not hold exactly one asset.
    WrongNumberOfAssets,
    /// The note's asset changed before the script ran.
    AssetAltered,
}

impl HtlcScriptError {
    /// All variants.
    pub const ALL: [Self; 5] = [
        Self::WrongNumberOfStorageItems,
        Self::WrongPreimage,
        Self::NotExpired,
        Self::WrongNumberOfAssets,
        Self::AssetAltered,
    ];

    /// The message, exactly as `masm/htlc.masm` defines it.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::WrongNumberOfStorageItems => "HTLC note expects exactly 13 note storage items",
            Self::WrongPreimage => "HTLC preimage does not hash to the hashlock",
            Self::NotExpired => "HTLC refund is possible only from the expiry block on",
            Self::WrongNumberOfAssets => "HTLC note must hold exactly one asset",
            Self::AssetAltered => {
                "HTLC note asset differs from the asset the note was created with"
            }
        }
    }

    /// The error code that the VM reports when this assertion fails.
    #[must_use]
    pub fn error_code(self) -> u64 {
        error_code_from_msg(self.message()).as_canonical_u64()
    }

    /// Returns the error for a VM error code, if the code belongs to the HTLC script.
    #[must_use]
    pub fn from_error_code(code: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|error| error.error_code() == code)
    }

    /// Finds an HTLC script error anywhere in an error chain, by message or by error code.
    #[must_use]
    pub fn find_in(err: &(dyn std::error::Error + 'static)) -> Option<Self> {
        let mut current = Some(err);
        while let Some(cause) = current {
            let text = cause.to_string();
            if let Some(error) = Self::ALL
                .into_iter()
                .find(|error| text.contains(error.message()))
            {
                return Some(error);
            }
            if let Some(error) = text
                .split("error code: ")
                .nth(1)
                .and_then(|rest| {
                    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                    digits.parse().ok()
                })
                .and_then(Self::from_error_code)
            {
                return Some(error);
            }
            current = cause.source();
        }
        None
    }
}

impl fmt::Display for HtlcScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for HtlcScriptError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_compiles() {
        let _ = htlc_script();
    }

    #[test]
    fn messages_match_the_script() {
        for error in HtlcScriptError::ALL {
            let definition = format!("=\"{}\"", error.message());
            assert!(
                HTLC_MASM.contains(&definition),
                "not defined in htlc.masm: {error}"
            );
        }
    }

    #[test]
    fn error_codes_round_trip() {
        for error in HtlcScriptError::ALL {
            assert_eq!(
                HtlcScriptError::from_error_code(error.error_code()),
                Some(error)
            );
        }
        // The code observed on testnet for a wrong preimage.
        assert_eq!(
            HtlcScriptError::from_error_code(16_002_105_711_497_080_711),
            Some(HtlcScriptError::WrongPreimage)
        );
    }

    #[test]
    fn attachment_scheme_matches_the_script() {
        let definition =
            format!("const HTLC_PREIMAGE_ATTACHMENT_SCHEME={PREIMAGE_ATTACHMENT_SCHEME}");
        assert!(HTLC_MASM.contains(&definition));
    }
}
