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

`contract.circuits().with_coin_encryption_keys([recipient])` supplies the recipient's encryption key for the call. The SDK then attaches the discovery ciphertext to each circuit-created output that the recipient's coin public key owns. The recipient's wallet then finds the coin on its next sync.

## Run

Start the devnet (node + indexer) from the repository root:

```bash
make dev-up   # from the repo root
```

`make dev-up` waits for the chain to produce a block past genesis. Starting the compose file directly is not enough: a transaction built while only genesis exists carries an expired TTL and the node rejects it.

Run the example:

```bash
make run-shielded-mint   # from the repo root
```

Output:

```
=== Midnight Shielded Mint Example ===

0. Syncing minter wallet...
   synced.

1. Recipient shielded address: mn_shield-addr_undeployed1...

2. Deploying mint contract...
   address: ...

3. Minting 1000 units to the recipient...
   minted.

4. Recipient syncing to discover the coin...
   discovered minted coin: token_type=..., value=1000

=== Done: recipient found the coin through normal sync ===
```

The minter and recipient are two independent wallets (genesis seeds `...0001` and `...0002`). The recipient never watches for the contract or the coin. It waits on its background sync until the coin is there.

Stop the devnet (from the repo root):

```bash
make dev-down
```

## Troubleshooting

If the node rejects a deploy or a mint with `custom error: 196` (`DustDoubleSpend`), a previous spend of the same Dust UTXO is still settling. This is usually transient: re-run the example. If the error persists, or if the node returns `171` (`OutOfDustValidityWindow`), the local devnet's fee-token (Dust) state is stale. Restart the devnet to reset it to a fresh genesis:

```bash
make dev-down && make dev-up
```

## Recompile the contract

The contract source lives in [`devnet/contracts/shielded-mint`](../../devnet/contracts/shielded-mint). `make run-shielded-mint` first compiles each devnet contract whose output is missing or out of date. So a change to `shielded-mint.compact` or to `COMPACT_REV` in the root [`Makefile`](../../Makefile) takes effect on the next run. The `Makefile` runs the Compact compiler fork that `COMPACT_REV` names from its image, so that step needs Docker ([Compile a contract](../../README.md#compile-a-contract) gives the details). Only that fork writes the analyzed IR that the interpreter runs. To recompile every contract, run this from the root of the repository:

```bash
make compile-contracts  # recompile every contract in devnet/contracts
```

The compiled artifacts keep the layout that [`devnet/contracts/README.md`](../../devnet/contracts/README.md#layout) describes. A deploy on chain needs the keys in `keys/`.
