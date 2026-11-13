//! Claim: alice locks tokens for bob, a third party claims them with the secret, bob collects.
//!
//! Run: `cargo run --release --example 01_claim` (after `00_setup`)
//!
//! 1. Alice picks a random secret and locks 0.1 token in a public HTLC note for bob.
//! 2. The relayer knows only the note ID and the secret. It fetches the public note from the node,
//!    checks that it is an HTLC with the agreed terms, and claims it. The script creates a P2ID
//!    note for bob, moves the tokens into it, attaches the secret, and seals it. The relayer pays
//!    only the fee: the tokens never enter its vault.
//! 3. Anyone can now read the secret from the chain. This example reads it from the node with a
//!    bare gRPC client, the way the other side of a cross-chain swap would.
//! 4. Bob consumes the payout note.

mod common;

use std::time::Duration;

use anyhow::{Context, ensure};
use common::{
    consume_all, fee_token, hex, show, testnet_client, token_balance, wait_for_commit, wallet,
};
use miden_client::asset::FungibleAsset;
use miden_client::block::BlockNumber;
use miden_client::crypto::FeltRng;
use miden_client::note::{Note, NoteFile};
use miden_client::rpc::domain::note::FetchedNote;
use miden_client::rpc::{Endpoint, GrpcClient, NodeRpcClient};
use miden_client::store::NoteFilter;
use miden_htlc::{HtlcNote, HtlcTerms, Payout, Preimage, revealed_preimage};

/// 0.1 fee token (6 decimals).
const AMOUNT: u64 = 100_000;
/// A refund becomes possible after this many blocks (about 2.5 minutes).
const EXPIRY_BLOCKS: u32 = 50;
const TIMEOUT: Duration = Duration::from_mins(3);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut alice_client = testnet_client("alice").await?;
    let mut bob_client = testnet_client("bob").await?;
    let mut relayer_client = testnet_client("relayer").await?;
    for client in [&mut alice_client, &mut bob_client, &mut relayer_client] {
        client.sync_state().await?;
    }
    let alice = wallet(&mut alice_client).await?;
    let bob = wallet(&mut bob_client).await?;
    let relayer = wallet(&mut relayer_client).await?;
    let fee = fee_token(&alice_client).await?;

    // 1. ALICE LOCKS
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

    println!("alice locks {AMOUNT} for bob {}", show(bob));
    println!("  hashlock {}", terms.hashlock);
    println!("  expiry   block {}", terms.expiry);
    let tx = alice_client
        .submit_new_transaction(alice, htlc.lock_request()?)
        .await?;
    let block = wait_for_commit(&mut alice_client, tx, TIMEOUT).await?;
    println!(
        "  HTLC note {} committed in block {block}",
        htlc.id().to_hex()
    );

    // 2. THE RELAYER CLAIMS, knowing only the note ID and the secret
    relayer_client
        .import_notes(&[NoteFile::NoteId(htlc.id())])
        .await?;
    relayer_client.sync_state().await?;
    let fetched: Note = relayer_client
        .get_input_note(htlc.id())
        .await?
        .context("the relayer imported the HTLC note")?
        .try_into()?;
    let fetched = HtlcNote::from_note(fetched)?;
    ensure!(
        *fetched.terms() == terms,
        "the HTLC note does not have the agreed terms"
    );
    println!(
        "\nrelayer fetched the public note from the node; it is an HTLC with the agreed terms"
    );

    let relayer_before = token_balance(&relayer_client, relayer, fee).await?;
    let tx = relayer_client
        .submit_new_transaction(relayer, fetched.claim_request(&secret)?)
        .await?;
    let block = wait_for_commit(&mut relayer_client, tx, TIMEOUT).await?;
    let relayer_fee = relayer_before - token_balance(&relayer_client, relayer, fee).await?;
    println!("relayer claimed in block {block}, paying only its fee ({relayer_fee})");

    // 3. ANYONE READS THE SECRET FROM THE CHAIN
    let expected = htlc.payout_recipient(Payout::Claim).digest();
    let payout_id = relayer_client
        .get_output_notes(NoteFilter::All)
        .await?
        .into_iter()
        .find(|record| record.recipient_digest() == expected)
        .context("the claim created the predicted payout note")?
        .id();

    let rpc = GrpcClient::new(&Endpoint::testnet(), 10_000);
    let (genesis, _) = rpc
        .get_block_header_by_number(Some(BlockNumber::from(0u32)), false)
        .await?;
    rpc.set_genesis_commitment(genesis.commitment()).await?;
    let revealed = match rpc.get_note_by_id(payout_id).await? {
        FetchedNote::Public(note, _) => revealed_preimage(note.attachments())?,
        FetchedNote::Private(_, _, attachments, _) => revealed_preimage(&attachments)?,
    }
    .context("the payout note carries the preimage attachment")?;
    println!(
        "\npayout note {} on chain carries the secret:",
        payout_id.to_hex()
    );
    println!("  revealed {}", hex(revealed.as_bytes()));
    println!("  matches the secret: {}", revealed == secret);
    println!(
        "  unlocks the hashlock: {}",
        terms.hashlock.is_unlocked_by(&revealed)
    );

    // 4. BOB COLLECTS
    let block = consume_all(&mut bob_client, bob, TIMEOUT).await?;
    let balance = token_balance(&bob_client, bob, fee).await?;
    println!("\nbob consumed the payout in block {block}; bob balance {balance}");
    Ok(())
}
