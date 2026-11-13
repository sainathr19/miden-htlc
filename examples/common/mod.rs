//! Testnet client setup and small helpers the examples share. Not part of the library.
//!
//! Each party has its own client, with its own store and keys, in `data/<party>/`:
//! - `alice`: locks funds in an HTLC and receives refunds.
//! - `bob`: the HTLC recipient.
//! - `relayer`: a third party that claims or refunds on someone else's behalf.

// Each example uses only some of these helpers.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use miden_client::Client;
use miden_client::account::component::BasicWallet;
use miden_client::account::{
    Account, AccountBuilder, AccountBuilderSchemaCommitmentExt, AccountId, AccountType,
};
use miden_client::address::NetworkId;
use miden_client::auth::{Approver, AuthSchemeId, AuthSecretKey, AuthSingleSig};
use miden_client::block::BlockNumber;
use miden_client::builder::ClientBuilder;
use miden_client::keystore::{FilesystemKeyStore, Keystore};
use miden_client::note::Note;
use miden_client::note::NoteConsumptionStatus;
use miden_client::store::{InputNoteRecord, TransactionFilter};
use miden_client::transaction::{TransactionId, TransactionRequestBuilder, TransactionStatus};
use miden_client_sqlite_store::SqliteStore;

/// The client type every example uses.
pub type TestnetClient = Client<FilesystemKeyStore>;

const POLL_EVERY: Duration = Duration::from_secs(3);

/// Returns the folder of one party's client.
pub fn data_dir(party: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(party)
}

/// Builds a testnet client for one party, with its store and keys in `data/<party>/`.
pub async fn testnet_client(party: &str) -> anyhow::Result<TestnetClient> {
    let dir = data_dir(party);
    std::fs::create_dir_all(&dir).context("create the data folder")?;
    let store = SqliteStore::new(dir.join("store.sqlite3"))
        .await
        .context("open the store")?;
    let client = ClientBuilder::<FilesystemKeyStore>::for_testnet()
        .store(Arc::new(store))
        .filesystem_keystore(dir.join("keys"))?
        .build()
        .await
        .context("build the client")?;
    Ok(client)
}

/// Returns the party's wallet: the first account its client tracks. Creates a public wallet with
/// one Falcon key if the client tracks none.
pub async fn wallet(client: &mut TestnetClient) -> anyhow::Result<AccountId> {
    if let Some((header, _)) = client.get_account_headers().await?.first() {
        return Ok(header.id());
    }
    Ok(new_wallet(client).await?.id())
}

/// Creates a public wallet with one Falcon key and tracks it.
pub async fn new_wallet(client: &mut TestnetClient) -> anyhow::Result<Account> {
    let secret_key = AuthSecretKey::new_falcon512_poseidon2();
    let auth = AuthSingleSig::new(Approver::new(
        secret_key.public_key().to_commitment(),
        AuthSchemeId::Falcon512Poseidon2,
    ));
    let seed: [u8; 32] = rand::random();
    let account = AccountBuilder::new(seed)
        .account_type(AccountType::Public)
        .with_component(auth)
        .with_component(BasicWallet)
        .build_with_schema_commitment()?;
    let keystore = client
        .authenticator()
        .expect("the client has a keystore")
        .clone();
    keystore.add_key(&secret_key, account.id()).await?;
    client.add_account(&account, false).await?;
    Ok(account)
}

/// Formats an account ID as hex and bech32.
pub fn show(id: AccountId) -> String {
    format!("{id} ({})", id.to_bech32(NetworkId::Testnet))
}

/// Returns the faucet ID of the token that pays transaction fees.
pub async fn fee_token(client: &TestnetClient) -> anyhow::Result<AccountId> {
    let header = client.get_latest_block_header().await?;
    let config = client
        .get_protocol_config(header.protocol_config_commitment())
        .await?;
    Ok(config.fee_asset_id().faucet_id())
}

/// Returns how many base units of `token` the account holds, per the local store.
pub async fn token_balance(
    client: &TestnetClient,
    account: AccountId,
    token: AccountId,
) -> anyhow::Result<u64> {
    let vault = client.get_account_vault(account).await?;
    Ok(vault
        .assets()
        .filter(|asset| asset.is_fungible() && asset.faucet_id() == token)
        .map(|asset| asset.unwrap_fungible().amount().as_u64())
        .sum())
}

/// Syncs until the transaction is committed, and returns its block.
pub async fn wait_for_commit(
    client: &mut TestnetClient,
    tx: TransactionId,
    timeout: Duration,
) -> anyhow::Result<BlockNumber> {
    let started = Instant::now();
    loop {
        client.sync_state().await?;
        let records = client
            .get_transactions(TransactionFilter::Ids(vec![tx]))
            .await?;
        match records.first().map(|record| &record.status) {
            Some(TransactionStatus::Committed { block_number, .. }) => return Ok(*block_number),
            Some(TransactionStatus::Discarded(cause)) => bail!("transaction discarded: {cause:?}"),
            _ if started.elapsed() > timeout => {
                bail!("transaction still pending after {timeout:?}")
            }
            _ => tokio::time::sleep(POLL_EVERY).await,
        }
    }
}

/// Syncs until the account has at least one note it can consume now, and returns those notes.
///
/// Screening runs without keys, so a signature-protected account gets
/// `ConsumableWithAuthorization` instead of `Consumable`. Both are accepted.
pub async fn wait_for_consumable_notes(
    client: &mut TestnetClient,
    account: AccountId,
    timeout: Duration,
) -> anyhow::Result<Vec<InputNoteRecord>> {
    let started = Instant::now();
    loop {
        client.sync_state().await?;
        let ready: Vec<InputNoteRecord> = client
            .get_consumable_notes(Some(account))
            .await?
            .into_iter()
            .filter(|(_, statuses)| {
                statuses.iter().any(|(id, status)| {
                    *id == account
                        && matches!(
                            status,
                            NoteConsumptionStatus::Consumable
                                | NoteConsumptionStatus::ConsumableWithAuthorization
                        )
                })
            })
            .map(|(record, _)| record)
            .collect();
        if !ready.is_empty() {
            return Ok(ready);
        }
        if started.elapsed() > timeout {
            bail!("no consumable note for {account} after {timeout:?}");
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Syncs until the client has seen block `height`.
pub async fn wait_until_block(
    client: &mut TestnetClient,
    height: BlockNumber,
) -> anyhow::Result<()> {
    loop {
        if client.sync_state().await?.block_num >= height {
            return Ok(());
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Waits for notes the account can consume now, consumes all of them in one transaction, and
/// returns the block that committed it.
pub async fn consume_all(
    client: &mut TestnetClient,
    account: AccountId,
    timeout: Duration,
) -> anyhow::Result<BlockNumber> {
    let records = wait_for_consumable_notes(client, account, timeout).await?;
    let notes: Vec<Note> = records
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<_, _>>()?;
    let request = TransactionRequestBuilder::new().build_consume_notes(notes)?;
    let tx = client.submit_new_transaction(account, request).await?;
    wait_for_commit(client, tx, timeout).await
}

/// Formats bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            write!(out, "{byte:02x}").expect("writing to a String does not fail");
            out
        })
}
