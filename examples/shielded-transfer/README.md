# Shielded Transfer Example

Self-transfer of 1 unit of a shielded token, end to end: sync wallet → build → submit → wait for finalization → start a background sync → wait for the outputs → print the post-balance.

**Local devnet only.** The public preprod faucet only funds *unshielded* addresses (NIGHT), and the SDK has no `unshielded → shielded` conversion helper. The local dev preset of `midnightntwrk/midnight-node` mints several shielded test tokens to the hardcoded dev seed at genesis. The example spends one of them, so it needs no external funding step. For the full token model see [`docs/tokens.md`](../../docs/tokens.md).

## Setup

Start the local devnet (`devnet/docker-compose.yml`):

```bash
make dev-up   # from the repo root
```

`make dev-up` waits for the chain to produce a block past genesis. Starting the compose file directly is not enough: a transaction built while only genesis exists carries an expired TTL and the node rejects it.

The example connects to the local devnet. To override an endpoint or the network, set these environment variables:

- `MIDNIGHT_NODE_URL`: the node, by default `ws://127.0.0.1:9944`
- `MIDNIGHT_INDEXER_URL`: the indexer, by default `http://127.0.0.1:8088`
- `MIDNIGHT_NETWORK`: the network, by default `undeployed`

## Run

```bash
cargo run -p example-shielded-transfer
```

Output (approximately):

```
=== Midnight Shielded Transfer ===

Network:           undeployed
Node:              ws://127.0.0.1:9944
Indexer:           http://127.0.0.1:8088

Syncing wallet from indexer (zswap and unshielded, then dust)...
Sync complete.
Shielded address:  mn_shield-addr_undeployed1...

--- Pre-transfer shielded balance ---
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000

Building shielded self-transfer: 1 unit of token ... back to own address
Building + submitting...
Submitted: ext hash ...
Best:      ...
Finalized: ...

Waiting for the wallet to see the outputs...

--- Post-transfer shielded balance ---
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 50000000000000
  00000000…: 49999999999999     # change from the spent input
  00000000…: 50000000000000
  00000000…: 1                  # the 1-unit self-transfer output
  00000000…: 50000000000000

--- Dust (paid the fee) ---
balance: ... SPECK, spendable UTXOs: ...

=== Done ===
```

## What it covers

- `Wallet::from_seed(DEV_SEED)?.with_network(&network).sync(&provider, &indexer_url)` + `MidnightProvider::with_wallet`: the first wallet sync against the indexer, then the attach step. The example attaches an `Arc<LocalWallet>`, so it keeps a handle on the wallet.
- `Wallet::shielded_address`: the synced wallet's own address, which receives the self-transfer
- `MidnightProvider::balance`: shielded coin enumeration (token ids are opaque; see [`docs/tokens.md`](../../docs/tokens.md))
- `MidnightProvider::transfer_shielded(token_type, amount, recipient)`: builds a proven zswap transfer and submits it
- `PendingTx::wait_best` + `wait_finalized`: the submission lifecycle
- `LocalWallet::sync_in_background`: resync the attached wallet in a background task until the handle drops
- `BackgroundSync::wait_until` with a balance check: wait until the wallet sees the outputs of the transfer
- `BackgroundSync::last_error`: after a timeout, the error of the last pass, when that pass failed

For the wallet API reference (sync, balances, transfers, Dust, persistence), see [`docs/wallet.md`](../../docs/wallet.md).
