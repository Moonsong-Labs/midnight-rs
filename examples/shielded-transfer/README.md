# Shielded Transfer Example

Self-transfer of 1 unit of a shielded token, end to end: sync wallet → build
→ submit → wait for finalization → resync and print post-balance.

**Local devnet only.** The public preprod faucet only funds *unshielded*
addresses (NIGHT), and the SDK has no `unshielded → shielded` conversion
helper today. The local dev preset of `midnightntwrk/midnight-node` mints
several shielded test tokens to the hardcoded dev seed at genesis, so the
example can spend one of them without any external funding step. For the full
token model see [`docs/tokens.md`](../../docs/tokens.md).

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
Shielded address:  mn_shield-addr_undeployed1...
Node:              ws://127.0.0.1:9944
Indexer:           http://127.0.0.1:8088

Syncing wallet from indexer (zswap + dust + unshielded in parallel)...
Sync complete.

--- Pre-transfer shielded balance ---
  ...00000000: 50000000000000
  ...00000000: 50000000000000
  ...

Building shielded self-transfer: 1 unit of token ...00000000 back to own address
Built: tx_bytes=13430

Submitting...
Submitted: ext hash ...
Best:      ...
Finalized: ...

Resyncing...

--- Post-transfer shielded balance ---
  ...00000000: 50000000000000
  ...00000000: 50000000000000
  ...
  ...00000000: 49999999999999     # change from the spent input
  ...00000000: 1                  # the 1-unit self-transfer output

--- Dust (paid the fee) ---
balance: ... SPECK, spendable UTXOs: ...

=== Done ===
```

## What it covers

- `Wallet::sync` + `MidnightProvider::with_wallet` — initial wallet sync against the indexer, then attach
- `MidnightProvider::balance` — shielded coin enumeration (token ids are opaque; see [`docs/tokens.md`](../../docs/tokens.md))
- `MidnightProvider::transfer_shielded(token_type, amount, recipient)` — builds a proven zswap transfer
- `MidnightProvider::submit` + `PendingTx::wait_best` + `wait_finalized` — submission lifecycle
- `MidnightProvider::resync_wallet` — incremental refresh to observe the new state

For the wallet API reference (sync, balances, transfers, Dust, persistence),
see [`docs/wallet.md`](../../docs/wallet.md).
