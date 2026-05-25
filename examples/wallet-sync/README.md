# Wallet Sync Example

Connects to any Midnight network, syncs the wallet's three legs (zswap shielded coins,
unshielded UTXOs, Dust) with progress reporting, prints addresses + balances + Dust
parameters, and optionally registers Dust or submits an unshielded self-transfer.

Uses a hard-coded seed so addresses stay stable across runs — do **not** reuse it.

## Setup

Pick a network and export its URLs once:

```bash
# Preprod
export MIDNIGHT_NODE_URL="wss://rpc.preprod.midnight.network"
export MIDNIGHT_INDEXER_URL="https://indexer.preprod.midnight.network"
export MIDNIGHT_NETWORK="preprod"   # defaults to preprod if unset

# OR — local devnet: `docker compose up -d` from the repo root (docker-compose.yml).
# Override the seed to point at the dev preset's prefunded wallet (`0000…0001`)
# so the balance + dust output is non-empty.
export MIDNIGHT_NODE_URL="ws://127.0.0.1:9944"
export MIDNIGHT_INDEXER_URL="http://127.0.0.1:8088"
export MIDNIGHT_NETWORK="undeployed"
export MIDNIGHT_WALLET_SEED="0000000000000000000000000000000000000000000000000000000000000001"
```

The example's default seed targets preprod. Set `MIDNIGHT_WALLET_SEED` to any
64-char hex string to point at a different wallet (e.g. the devnet prefunded
seed above).

For preprod, fund the example's unshielded address via [the preprod faucet](https://faucet.preprod.midnight.network/):

```
mn_addr_preprod1cu74c4snt48ztvvjfhlgjx64ydqy25y682ujtjde034l36umcxfsg697rj
```

## Run

```bash
# Balance only (no transactions submitted)
cargo run --release -p example-wallet-sync

# + one-time Dust registration
REGISTER_DUST=1 cargo run --release -p example-wallet-sync

# + unshielded self-transfer of N STAR (atomic NIGHT units).
# Requires Dust sync to have finished — fees come from real Dust UTXOs.
TRANSFER_AMOUNT=100 cargo run --release -p example-wallet-sync
```

## What it covers

- `MidnightProvider::sync_wallet(...).stream()` — streamed `SyncProgress` events
- `provider.balance()` — three asset legs (shielded coins, unshielded UTXOs, Dust)
- `provider.wallet()` — lower-level read access for parameters and sync cursors
- `provider.register_dust(None)` — one-time Dust registration
- `provider.transfer_unshielded(NIGHT, amount, recipient)` — self-transfer
- `provider.submit(tx_bytes)` + `PendingTx::wait_best` — submission lifecycle

For the API reference, see [`docs/wallet.md`](../../docs/wallet.md).

## Note

Dust sync from genesis can take 30+ minutes on a mainnet-scale history. Progress is
checkpointed to disk under `~/.midnight/wallets/{network}/{seed_hash}/`, so reruns
resume from the last cursor.
