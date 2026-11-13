//! Runs the HTLC note script on a local mock chain, with no network and no fees.
//!
//! Alice locks tokens for Bob. A relayer, a third party, claims or refunds.

use std::collections::BTreeMap;

use anyhow::Context;
use miden_client::account::AccountId;
use miden_client::asset::{Asset, FungibleAsset};
use miden_client::auth::AuthSchemeId;
use miden_client::block::BlockNumber;
use miden_client::transaction::{ExecutedTransaction, RawOutputNote};
use miden_client::{Felt, Word};
use miden_htlc::{HtlcNote, HtlcScriptError, HtlcTerms, Payout, Preimage, revealed_preimage};
use miden_testing::{Auth, MockChain};

const AMOUNT: u64 = 100;
const EXPIRY_BLOCK: u32 = 5;

struct Fixture {
    chain: MockChain,
    alice: AccountId,
    relayer: AccountId,
    htlc: HtlcNote,
    asset: Asset,
    secret: Preimage,
}

fn auth() -> Auth {
    Auth::BasicAuth {
        auth_scheme: AuthSchemeId::Falcon512Poseidon2,
    }
}

/// A chain where alice's HTLC note for bob is already committed.
fn fixture() -> anyhow::Result<Fixture> {
    let mut builder = MockChain::builder();
    let faucet = builder.add_existing_basic_faucet(auth(), "HTLC", 1_000_000, None)?;
    let alice = builder.add_existing_wallet(auth())?.id();
    let bob = builder.add_existing_wallet(auth())?.id();
    let relayer = builder.add_existing_wallet(auth())?.id();

    let asset: Asset = FungibleAsset::new(faucet.id(), AMOUNT)?.into();
    let secret = Preimage::new([42; 32]);
    let terms = HtlcTerms {
        hashlock: secret.hashlock(),
        recipient: bob,
        refund: alice,
        expiry: BlockNumber::from(EXPIRY_BLOCK),
    };
    let serial = Word::from([
        Felt::from(1u32),
        Felt::from(2u32),
        Felt::from(3u32),
        Felt::from(4u32),
    ]);
    let htlc = HtlcNote::new(alice, terms, asset, serial)?;
    builder.add_output_note(RawOutputNote::Full(htlc.note().clone()));

    Ok(Fixture {
        chain: builder.build()?,
        alice,
        relayer,
        htlc,
        asset,
        secret,
    })
}

impl Fixture {
    /// The relayer consumes the HTLC note. With a preimage, it claims; without one, it refunds.
    async fn consume(&self, preimage: Option<&Preimage>) -> anyhow::Result<ExecutedTransaction> {
        let mut tx = self
            .chain
            .build_transaction(self.relayer)
            .authenticated_input_note(self.htlc.id());
        if let Some(preimage) = preimage {
            let (key, value) = preimage.claim_advice();
            tx = tx
                .extend_note_args(BTreeMap::from([(self.htlc.id(), key)]))
                .add_advice_map_entry(key, value);
        }
        Ok(tx.build()?.execute().await?)
    }

    /// Returns the output note that the given payout path creates, and checks it holds the asset.
    fn payout<'tx>(
        &self,
        executed: &'tx ExecutedTransaction,
        payout: Payout,
    ) -> anyhow::Result<&'tx RawOutputNote> {
        let expected = self.htlc.payout_recipient(payout).digest();
        let note = executed
            .output_notes()
            .iter()
            .find(|note| note.recipient_digest() == expected)
            .context("the transaction created the expected payout note")?;
        anyhow::ensure!(
            note.assets().iter().copied().eq([self.asset]),
            "the payout note holds the HTLC asset"
        );
        Ok(note)
    }
}

/// Executes a consume that must fail, and returns the HTLC script error it failed with.
async fn expect_script_error(
    fixture: &Fixture,
    preimage: Option<&Preimage>,
) -> anyhow::Result<HtlcScriptError> {
    let err = fixture
        .consume(preimage)
        .await
        .err()
        .context("the transaction must fail")?;
    HtlcScriptError::find_in(err.as_ref())
        .with_context(|| format!("not an HTLC script error: {err:#}"))
}

#[tokio::test]
async fn third_party_claim_pays_the_recipient_and_reveals_the_secret() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let executed = fixture.consume(Some(&fixture.secret)).await?;

    let payout = fixture.payout(&executed, Payout::Claim)?;
    assert_eq!(
        revealed_preimage(payout.attachments())?,
        Some(fixture.secret)
    );
    assert!(
        executed.account_patch().vault().is_empty(),
        "the asset never enters the relayer's vault"
    );
    Ok(())
}

#[tokio::test]
async fn claim_with_a_wrong_secret_fails() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let wrong = Preimage::new([7; 32]);
    assert_eq!(
        expect_script_error(&fixture, Some(&wrong)).await?,
        HtlcScriptError::WrongPreimage
    );
    Ok(())
}

#[tokio::test]
async fn refund_before_expiry_fails() -> anyhow::Result<()> {
    let fixture = fixture()?;
    assert_eq!(
        expect_script_error(&fixture, None).await?,
        HtlcScriptError::NotExpired
    );
    Ok(())
}

#[tokio::test]
async fn refund_after_expiry_pays_the_refund_account() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    fixture.chain.prove_until_block(EXPIRY_BLOCK)?;
    let executed = fixture.consume(None).await?;

    let payout = fixture.payout(&executed, Payout::Refund)?;
    assert_eq!(
        revealed_preimage(payout.attachments())?,
        None,
        "a refund reveals nothing"
    );
    assert_eq!(fixture.htlc.terms().refund, fixture.alice);
    Ok(())
}

#[tokio::test]
async fn claim_after_expiry_is_still_allowed() -> anyhow::Result<()> {
    let mut fixture = fixture()?;
    fixture.chain.prove_until_block(EXPIRY_BLOCK + 1)?;
    let executed = fixture.consume(Some(&fixture.secret)).await?;
    fixture.payout(&executed, Payout::Claim)?;
    Ok(())
}

#[tokio::test]
async fn a_note_with_another_script_is_not_an_htlc() -> anyhow::Result<()> {
    let fixture = fixture()?;
    let mut builder = MockChain::builder();
    let sender = builder.add_existing_wallet(auth())?.id();
    let p2id = builder.add_p2id_note(
        sender,
        fixture.alice,
        &[fixture.asset],
        miden_client::note::NoteType::Public,
    )?;
    assert!(matches!(
        HtlcNote::from_note(p2id),
        Err(miden_htlc::HtlcError::NotHtlcScript)
    ));
    assert_eq!(
        HtlcNote::from_note(fixture.htlc.note().clone())?,
        fixture.htlc
    );
    Ok(())
}
