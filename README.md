# miden-htlc

[![CI](https://github.com/sainathr19/miden-htlc/actions/workflows/ci.yml/badge.svg)](https://github.com/sainathr19/miden-htlc/actions/workflows/ci.yml)

A hash time-locked contract (HTLC) note for the [Miden](https://miden.xyz) rollup, written in Miden Assembly and tested on Miden testnet with `miden-client` 0.18.

An HTLC locks tokens so that:

- **anyone** who knows a secret can release them to a fixed **recipient**, and
- after an expiry block, **anyone** can return them to a fixed **refund** account.

The secret becomes public when it is used. That is what makes HTLCs the building block for atomic swaps across chains.

## How it works

The HTLC is a public note. Its script is in [`masm/htlc.masm`](masm/htlc.masm).

**Storage (13 felts):**

| Felts | Field |
|---|---|
| 0–7 | Hashlock: SHA-256 of the 32-byte secret, as 8 big-endian u32 limbs |
| 8, 9 | Recipient account ID (suffix, prefix) |
| 10, 11 | Refund account ID (suffix, prefix) |
| 12 | Expiry block number |

**Assets:** exactly one fungible asset.

**Claim path** (the note args are non-empty, at any block):

1. The note args are a key into the advice map, where the consumer puts the secret.
2. The script checks that `sha256(secret) == hashlock`.
3. It creates a **public P2ID note for the recipient** and moves the asset straight into it.
4. It **attaches the secret** to that note (attachment scheme `1000`). Attachments are always public.
5. It **seals** the note, so the consumer cannot change it.

**Refund path** (empty note args):

1. The script checks that the reference block is at or after the expiry block.
2. It creates and seals a public P2ID note for the refund account, holding the asset.

**Guarantees:**

- **Any account with the standard wallet can claim or refund,** for example a relayer or a watchtower.
- **The consumer only pays the fee.** The asset never enters its vault.
- **The secret is always published on a claim.** This holds even when a private account claims.
- **A claim after expiry is still allowed,** until someone refunds. The first transaction to land wins.
- **The note's asset must be unchanged.** The same guard as the standard PSWAP note.

## Run it on testnet

```bash
cargo run --release --example 00_setup     # create alice, bob, relayer; fund alice from the faucet
cargo run --release --example 01_claim     # lock, third-party claim, read the secret from the chain, bob collects
cargo run --release --example 02_refund    # wrong secret and early refund are rejected, then refund after expiry
```

Each party has its own client, with its own store and keys, in `data/<party>/`. That folder holds secret keys and is ignored by git. Always use `--release`: the Miden VM is very slow in debug builds.

## Use it as a library

```rust
use miden_htlc::{HtlcNote, HtlcTerms, Preimage};

let secret = Preimage::new(rand::random());
let terms = HtlcTerms { hashlock: secret.hashlock(), recipient: bob, refund: alice, expiry };
let htlc = HtlcNote::new(alice, terms, asset, serial)?;

client.submit_new_transaction(alice, htlc.lock_request()?).await?;     // alice locks
relayer.submit_new_transaction(r, htlc.claim_request(&secret)?).await?; // anyone claims
relayer.submit_new_transaction(r, htlc.refund_request()?).await?;       // anyone refunds after expiry
```

- **Check notes from others with `HtlcNote::from_note(note)`.** It verifies the script and reads the terms, so you can compare them with what you agreed.
- **`revealed_preimage(attachments)`** reads the secret from a claim's payout note.
- **`HtlcNote::payout_recipient(Payout::Claim | Payout::Refund)`** predicts the payout note, so you can recognise it.
- **`HtlcScriptError::find_in(&err)`** turns a failed transaction into the script's own reason, even when the node only reports an error code.

## Tests

```bash
cargo test --release
```

This runs without any network:

- **Unit tests.** They include checks that the Rust storage layout, error messages, attachment scheme and serial offsets match `masm/htlc.masm`, so the two cannot drift apart.
- **Mock-chain tests** (`tests/mock_chain.rs`). They execute the note script on a local Miden chain: third-party claim, wrong secret, refund before and after expiry, claim after expiry, and rejecting a non-HTLC note. They take under a second.

CI runs `cargo fmt --check`, `cargo clippy -D warnings` with the pedantic lints, and the tests.

## Results on testnet (2026-11-11)

```
alice locks 100000 for bob ...
relayer fetched the public note from the node and verified script and terms
relayer claimed in block 1035655, paying only its fee (119)
payout note 0x83e5… on chain carries the secret:
  matches the secret: true
  sha256(revealed) == hashlock: true
bob consumed the payout in block 1035659; bob balance 99881

claim with a wrong secret: REJECTED during local execution, nothing was sent
    reason: HTLC preimage does not hash to the hashlock
refund before expiry: REJECTED during local execution, nothing was sent
    reason: HTLC refund is possible only from the expiry block on
relayer refunded in block 1035796; the payout note goes to alice
alice balance 8899167 -> 8898929: the 100000 came back, she paid 2 fees (238)
```

## Things worth knowing

- **Note creation needs the consumer's `create_note`, but assets don't.** A note script cannot create a note itself; it calls the consuming account's `wallet::create_note`. It can, however, add its own asset and an attachment to that note directly, and seal it.
- **A note script fetched from the node carries no error messages.** A failed assertion then reports only a numeric code. The code is a hash of the message (`miden_client::vm::error_code_from_msg`), so `HtlcScriptError::from_error_code` maps it back.
- **The note cannot check its own terms when it is created.** Its script runs only when it is consumed. Whoever relies on an HTLC note must verify the script root and the terms first. With a public note this is cheap: fetch the note by ID and compare.
- **The recipient needs two transactions:** the claim creates the payout note, and the recipient then consumes it.
- **A spent nullifier does not say whether the note was claimed or refunded.** The payout note does: only a claim carries the secret.

## Not done yet

- Private HTLC notes: amount, parties and hashlock hidden from everyone but the participants.
- An early refund (reject) initiated by the recipient.
- A watcher that finds the payout note from the HTLC nullifier, without knowing who claimed.

## License

MIT
