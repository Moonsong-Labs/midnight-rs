# Shielded Transfer Example

Self-transfer of 1 unit of a shielded token, end to end: sync wallet → build → submit → wait for finalization → resync and print post-balance.

**Local devnet only.** The public preprod faucet only funds *unshielded* addresses (NIGHT), and the SDK has no `unshielded → shielded` conversion helper. The local dev preset of `midnightntwrk/midnight-node` mints several shielded test tokens to the hardcoded dev seed at genesis. The example spends one of them, so it needs no external funding step. For the full token model see [`docs/tokens.md`](../../docs/tokens.md).

## Setup

Start the local devnet (`devnet/docker-compose.yml`):

```bash
make dev-up   # from the repo root
```

`make dev-up` waits for the chain to produce a block past genesis. Starting the compose file directly is not enough: a transaction built while only genesis exists carries an expired TTL and the node rejects it.

Export the URLs:

```bash
export MIDNIGHT_NODE_URL="ws://127.0.0.1:9944"
export MIDNIGHT_INDEXER_URL="http://127.0.0.1:8088"
export MIDNIGHT_NETWORK="undeployed"   # defaults to undeployed if unset
```

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

Syncing wallet from indexer (zswap + dust + unshielded in parallel)...
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

Resyncing...

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

- `Wallet::from_seed(DEV_SEED)?.with_network(&network).sync(&provider)` + `MidnightProvider::with_wallet`: the first wallet sync against the indexer, then the attach step
- `Wallet::shielded_address`: the synced wallet's own address, which receives the self-transfer
- `MidnightProvider::balance`: shielded coin enumeration (token ids are opaque; see [`docs/tokens.md`](../../docs/tokens.md))
- `MidnightProvider::transfer_shielded(token_type, amount, recipient)`: builds a proven zswap transfer and submits it
- `PendingTx::wait_best` + `wait_finalized`: the submission lifecycle
- `MidnightProvider::resync_wallet`: incremental refresh to observe the new state

For the wallet API reference (sync, balances, transfers, Dust, persistence), see [`docs/wallet.md`](../../docs/wallet.md).
