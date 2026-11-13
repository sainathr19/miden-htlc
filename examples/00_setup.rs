//! Setup: create the three parties and fund them.
//!
//! Run: `cargo run --release --example 00_setup`
//!
//! - alice: locks funds in HTLCs. Funded from the testnet faucet.
//! - bob: the HTLC recipient. Needs no funds: the payout note pays his first fee.
//! - relayer: a third party that claims and refunds for others. Pays its own fees, so alice sends
//!   it some tokens.
//!
//! Running this again is safe: it creates and funds only what is missing.

mod common;

use std::time::Duration;

use common::{
    consume_all, fee_token, show, testnet_client, token_balance, wait_for_commit, wallet,
};
use miden_client::address::NetworkId;
use miden_client::asset::FungibleAsset;
use miden_client::note::NoteType;
use miden_client::transaction::{PaymentNoteDescription, TransactionRequestBuilder};

/// The relayer gets 1 fee token: enough for many claim and refund fees.
const RELAYER_FUNDING: u64 = 1_000_000;
const TIMEOUT: Duration = Duration::from_mins(10);

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
    println!("alice:   {}", show(alice));
    println!("bob:     {}", show(bob));
    println!("relayer: {}", show(relayer));

    if token_balance(&alice_client, alice, fee).await? == 0 {
        println!(
            "\nalice has no tokens. Open https://faucet.testnet.miden.io/, paste\n  {}\nand click \
             \"Send Public Note\". Waiting...",
            alice.to_bech32(NetworkId::Testnet)
        );
        consume_all(&mut alice_client, alice, TIMEOUT).await?;
    }

    if token_balance(&relayer_client, relayer, fee).await? == 0 {
        println!("\nfunding the relayer with {RELAYER_FUNDING}...");
        let payment = PaymentNoteDescription::new(
            vec![FungibleAsset::new(fee, RELAYER_FUNDING)?.into()],
            alice,
            relayer,
        );
        let request = TransactionRequestBuilder::new().build_pay_to_id(
            payment,
            NoteType::Public,
            alice_client.rng(),
        )?;
        let tx = alice_client.submit_new_transaction(alice, request).await?;
        wait_for_commit(&mut alice_client, tx, TIMEOUT).await?;
        consume_all(&mut relayer_client, relayer, TIMEOUT).await?;
    }

    println!(
        "\nalice balance:   {}",
        token_balance(&alice_client, alice, fee).await?
    );
    println!(
        "relayer balance: {}",
        token_balance(&relayer_client, relayer, fee).await?
    );
    println!(
        "bob balance:     {}",
        token_balance(&bob_client, bob, fee).await?
    );
    println!("setup complete");
    Ok(())
}
