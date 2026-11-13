//! Refund: the failure paths, then a refund after expiry.
//!
//! Run: `cargo run --release --example 02_refund` (after `00_setup`)
//! Takes about a minute: it waits for the expiry block on purpose.
//!
//! 1. Alice locks 0.1 token for bob, refundable from "now + 10 blocks".
//! 2. The relayer claims with a wrong secret. The script rejects it during local execution:
//!    nothing reaches the network, and no fee is paid.
//! 3. The relayer tries to refund before expiry. Rejected the same way.
//! 4. After the expiry block, the relayer refunds. The script creates a P2ID note for alice. Anyone
//!    can refund; the tokens can only go back to alice.
//! 5. Alice consumes the refund note.

mod common;

use std::time::Duration;

use anyhow::{Context, bail};
use common::{
    TestnetClient, consume_all, fee_token, testnet_client, token_balance, wait_for_commit,
    wait_until_block, wallet,
};
use miden_client::account::AccountId;
use miden_client::asset::FungibleAsset;
use miden_client::block::BlockNumber;
use miden_client::crypto::FeltRng;
use miden_client::note::{Note, NoteFile};
use miden_client::transaction::TransactionRequest;
use miden_htlc::{HtlcNote, HtlcScriptError, HtlcTerms, Preimage};

const AMOUNT: u64 = 100_000;
const EXPIRY_BLOCKS: u32 = 10;
const TIMEOUT: Duration = Duration::from_mins(3);

/// Executes a request locally, expects it to fail, and prints the script's reason.
async fn expect_rejected(
    client: &mut TestnetClient,
    account: AccountId,
    label: &str,
    request: TransactionRequest,
) -> anyhow::Result<()> {
    match client.execute_transaction(account, request).await {
        Ok(_) => bail!("{label}: expected a failure, but execution succeeded"),
        Err(err) => {
            let reason = HtlcScriptError::find_in(&err).map_or_else(
                || format!("not an HTLC error: {err}"),
                |error| error.to_string(),
            );
            println!("{label}: REJECTED during local execution, nothing was sent");
            println!("    reason: {reason}");
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut alice_client = testnet_client("alice").await?;
    let mut bob_client = testnet_client("bob").await?;
    let mut relayer_client = testnet_client("relayer").await?;
    for client in [&mut alice_client, &mut relayer_client] {
        client.sync_state().await?;
    }
    let alice = wallet(&mut alice_client).await?;
    let bob = wallet(&mut bob_client).await?;
    let relayer = wallet(&mut relayer_client).await?;
    let fee = fee_token(&alice_client).await?;

    // 1. ALICE LOCKS, with a short expiry
    let secret = Preimage::new(rand::random());
    let terms = HtlcTerms {
        hashlock: secret.hashlock(),
        recipient: bob,
        refund: alice,
        expiry: BlockNumber::from(alice_client.get_sync_height().await?.as_u32() + EXPIRY_BLOCKS),
    };
    let serial = alice_client.rng().draw_word();
    let htlc = HtlcNote::new(
        alice,
        terms,
        FungibleAsset::new(fee, AMOUNT)?.into(),
        serial,
    )?;
    let alice_before = token_balance(&alice_client, alice, fee).await?;
    let tx = alice_client
        .submit_new_transaction(alice, htlc.lock_request()?)
        .await?;
    let block = wait_for_commit(&mut alice_client, tx, TIMEOUT).await?;
    println!(
        "alice locked {AMOUNT} in HTLC {} at block {block}, expiry block {}",
        htlc.id().to_hex(),
        terms.expiry
    );

    relayer_client
        .import_notes(&[NoteFile::NoteId(htlc.id())])
        .await?;
    relayer_client.sync_state().await?;
    let note: Note = relayer_client
        .get_input_note(htlc.id())
        .await?
        .context("the relayer imported the HTLC note")?
        .try_into()?;
    let htlc = HtlcNote::from_note(note)?;

    // 2. WRONG SECRET. `claim_request` would refuse to build this; the unchecked variant shows that
    //    the note script rejects it too.
    let wrong = Preimage::new(rand::random());
    let request = htlc.claim_request_unchecked(&wrong)?;
    expect_rejected(
        &mut relayer_client,
        relayer,
        "claim with a wrong secret",
        request,
    )
    .await?;

    // 3. REFUND BEFORE EXPIRY
    let request = htlc.refund_request()?;
    expect_rejected(
        &mut relayer_client,
        relayer,
        "refund before expiry",
        request,
    )
    .await?;

    // 4. REFUND AFTER EXPIRY, by the relayer
    println!("waiting for block {}...", terms.expiry);
    wait_until_block(&mut relayer_client, terms.expiry).await?;
    let tx = relayer_client
        .submit_new_transaction(relayer, htlc.refund_request()?)
        .await?;
    let block = wait_for_commit(&mut relayer_client, tx, TIMEOUT).await?;
    println!("relayer refunded in block {block}; the payout note goes to alice");

    // 5. ALICE COLLECTS
    consume_all(&mut alice_client, alice, TIMEOUT).await?;
    let alice_after = token_balance(&alice_client, alice, fee).await?;
    println!(
        "alice balance {alice_before} -> {alice_after}: the {AMOUNT} came back, she paid 2 fees ({})",
        alice_before - alice_after
    );
    Ok(())
}
