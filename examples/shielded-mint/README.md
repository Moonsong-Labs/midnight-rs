# Shielded Mint Example

Deploys a token-minting contract to a local dev node and mints a shielded coin to an *external* wallet, which then discovers the coin through normal sync, no `watchFor` and no out-of-band coordination.

## Contract

```compact
import CompactStandardLibrary;

export circuit mint(domain_sep: Bytes<32>, value: Uint<64>, nonce: Bytes<32>, coinPK: ZswapCoinPublicKey): [] {
  mintShieldedToken(
    disclose(domain_sep),
    disclose(value),
    disclose(nonce),
    left<ZswapCoinPublicKey, ContractAddress>(disclose(coinPK))
  );
}
```

The recipient is wrapped with a **compile-time-constant** `left<ZswapCoinPublicKey, ContractAddress>(coinPK)`. This matters: `mintShieldedToken` takes an `Either<ZswapCoinPublicKey, ContractAddress>`, and a *runtime* `Either` would compile to a circuit that also carries the (untaken) contract-recipient branch. That branch's skipped public inputs hold non-zero recipient/commitment data the prover cannot reproduce with its zero padding, so verification fails with `InvalidProof`. Passing `coinPK` as a plain `ZswapCoinPublicKey` and wrapping it with a constant `left(...)` folds the branch away.

The minted coin's color is a custom shielded token type derived from `domain_sep` and the contract's own address, so each contract mints its own token.

## Discovery

A shielded coin is owned by a **coin public key** and discovered through an **encryption public key**. Circuits only ever see the coin public key, so by default the on-chain output the `mint` circuit creates carries no discovery ciphertext, and an external recipient would have to scan for it explicitly.

`Contract::with_coin_encryption_keys([(coin_pk, enc_pk)])` supplies the `coin_public_key -> encryption_public_key` mapping. The SDK then attaches the discovery ciphertext to each matching circuit-created output, so the recipient's wallet finds the coin on its next sync.

## Run

Start the devnet (node + indexer) from the repository root, then wait until both are serving:

```bash
docker compose -f devnet/docker-compose.yml up -d   # from the repo root
# node RPC
while ! curl -sf http://localhost:9944/health > /dev/null 2>&1; do sleep 2; done
# indexer (any HTTP response = port is up)
while ! curl -s --max-time 2 http://localhost:8088 > /dev/null 2>&1; do sleep 2; done
```

Run the example:

```bash
cargo run -p example-shielded-mint
```

Output:

```
=== Midnight Shielded Mint Example ===

0. Syncing minter wallet...
   synced.

1. Recipient shielded address: mn_shield-addr_undeployed1...

2. Deploying mint contract...
   address: 33847c1c636dee1fb065c80042d654c3cd5196eeccf8863b6aa732486bf088bc

3. Minting 1000 units to the recipient...
   minted.

4. Recipient syncing to discover the coin...
   discovered minted coin: token_type=fe11325c..., value=1000

=== Done: recipient found the coin through normal sync ===
```

The minter and recipient are two independent wallets (genesis seeds `...0001` and `...0002`). The recipient never watches for the contract or the coin; it just syncs and the coin is there.

Stop the devnet (from the repo root):

```bash
docker compose -f devnet/docker-compose.yml down
```

## Troubleshooting

If a deploy or mint is rejected with `custom error: 196` (`DustDoubleSpend`), a previous spend of the same Dust UTXO is still settling. This is usually transient: re-run the example. If it persists, or if you hit `171` (`OutOfDustValidityWindow`), the local devnet's fee-token (Dust) state has gone stale. Restart the devnet to reset it to a fresh genesis at the current time:

```bash
docker compose -f devnet/docker-compose.yml down && docker compose -f devnet/docker-compose.yml up -d
```

## Recompile the contract

The contract source and compiled artifacts live in [`devnet/contracts/shielded-mint`](../../devnet/contracts/shielded-mint). If you modify `shielded-mint.compact`, recompile with the [extended Compact compiler](https://github.com/RomarQ/compact/tree/feat/contract-info-extensions) (it emits the per-circuit `ir` field the interpreter needs). ZK keys are required for on-chain deployment:

```bash
cd ../../devnet/contracts/shielded-mint && compactc shielded-mint.compact compiled
```
