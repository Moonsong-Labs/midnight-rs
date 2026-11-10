# Wallet Sync Example

The example syncs a wallet against the local devnet, or another network that runs ledger 9. It syncs the wallet's three legs (zswap shielded coins, unshielded UTXOs, Dust) with progress reporting. It prints the addresses and the balances. As an option, it registers Dust or submits an unshielded self-transfer.

By default, the example uses the dev seed (`0000…0001`), which the local devnet funds at genesis. Do **not** use it on a public network.

## Setup

Start the local devnet from the repo root:

```bash
make dev-up
```

The example connects to the local devnet. To sync against another network that runs ledger 9, or another wallet, set these environment variables:

- `MIDNIGHT_NODE_URL`: the node, by default `ws://127.0.0.1:9944`
- `MIDNIGHT_INDEXER_URL`: the indexer, by default `http://127.0.0.1:8088`
- `MIDNIGHT_NETWORK`: the network, by default `undeployed`
- `MIDNIGHT_WALLET_SEED`: the seed, by default the dev seed

`MIDNIGHT_WALLET_SEED` takes any format that `Seed`'s `FromStr` accepts:

- 16-, 32- or 64-byte hex
- A BIP-39 mnemonic phrase (12/15/18/21/24 words), derived via the standard `Mnemonic::to_seed("")`. For a 25th-word passphrase, switch the example to call `Seed::from_mnemonic_with_passphrase(...)` directly.

## Run

```bash
# Balance only (no transactions submitted)
cargo run --release -p example-wallet-sync

# + Dust registration of one tNIGHT UTXO
REGISTER_DUST=1 cargo run --release -p example-wallet-sync

# + unshielded self-transfer of N STAR (atomic NIGHT units).
# Requires a finished Dust sync: the fees come from real Dust UTXOs.
TRANSFER_AMOUNT=100 cargo run --release -p example-wallet-sync
```

## What it covers

- `Wallet::from_seed(seed.clone()).with_network(&network).sync_with_progress(&provider, &indexer_url)`: streamed `SyncProgress` events, then `with_wallet(&wallet)` to attach a borrow. The sync checks a persisted snapshot's chain pin.
- `wallet.balance()`: the three asset legs (shielded coins, unshielded UTXOs, Dust)
- `provider.send_transaction(TransactionRequest::register_dust())`: registers one tNIGHT UTXO for Dust generation
- `provider.send_transaction(TransactionRequest::unshielded(NIGHT, amount, seed.unshielded_recipient()))`: an unshielded self-transfer
- `PendingTx::extrinsic_hash` and `PendingTx::get_receipt`: the submission lifecycle

For the API reference, see [`docs/wallet.md`](../../docs/wallet.md).

## Note

Dust sync from genesis can take 30+ minutes on a mainnet-scale history. Progress is checkpointed to disk under `~/.midnight/wallets/<network>/<sha256 of the unshielded address>/`, so reruns resume from the last cursor.
