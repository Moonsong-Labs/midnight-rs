# Token Model

A short, opinionated reference to the assets on Midnight and how the SDK
exposes them. Defers to the upstream specs for chain-level detail; the goal
here is to keep callers from confusing the two ledgers.

## Two ledgers, one type alias

Midnight maintains two independent ledgers for fungible tokens:

| Ledger | State | What lives there |
|---|---|---|
| **Unshielded** | UTXO set (`UtxoState`) | Public-balance assets keyed by owner address |
| **Shielded** (zswap) | Commitment + nullifier sets | Privacy-preserving coins |

Both refer to a token id as a 32-byte value (`RawTokenType` in the spec;
surfaced in the SDK as `UnshieldedTokenType` and `ShieldedTokenType`, which
are distinct newtypes wrapping the same `HashOutput([u8; 32])`).

The zswap spec is explicit about the relationship: *"Shielded and unshielded
tokens are not usually interchangeable."* An asset issued as an unshielded
UTXO of type `T` is a **different on-chain asset** from a shielded coin of
type `T`. Bridging the two requires a contract that mints in one ledger after
burning in the other; the SDK provides no built-in conversion.

## NIGHT

NIGHT is the chain's native unshielded token, defined in
[spec/night.md](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/night.md).
It lives exclusively in `UtxoState` and is the only asset that the
dust-generation mechanism recognizes for fee-availability accrual (see
[`dust-and-fees.md`](dust-and-fees.md)).

The SDK constant `midnight_wallet::NIGHT` is typed `UnshieldedTokenType` for
exactly this reason. **There is no shielded NIGHT.** A `ShieldedTokenType`
with the same underlying bytes (`HashOutput([0u8; 32])`) is a different asset
— conventionally the default test-token id on dev presets, with no semantic
relationship to NIGHT.

On testnet networks (preprod, devnet, undeployed) NIGHT is referred to as
**tNIGHT** — the `t` prefix stands for *test*, distinguishing testnet supply
from mainnet. Same protocol, same type, different network identity.

## Token cheat sheet

| Token | Rust type | Atomic unit | Lives in |
|---|---|---|---|
| NIGHT / tNIGHT | `UnshieldedTokenType` | STAR (1 NIGHT = 10⁶ STAR) | `WalletBalance::unshielded` |
| DUST | (fee-only; no transfer API) | SPECK (1 DUST = 10¹⁵ SPECK) | `WalletBalance::dust` |
| Shielded coins | `ShieldedTokenType` | token-defined | `WalletBalance::shielded.coins` |
| Contract-minted unshielded | `UnshieldedTokenType` | token-defined | `WalletBalance::unshielded` |

Contract-minted unshielded tokens derive their type id from
`hash(contract_address, pre_token)` — see the `unshielded_mints` effect in
[`intents-and-zswap-mechanics.md`](intents-and-zswap-mechanics.md).

## Where shielded coins come from

A shielded coin can only exist if one of the following created it:

- **Genesis allocation.** Local devnet presets pre-mint several shielded test
  tokens to known dev seeds — that's why `WalletBalance::shielded.coins` is
  non-empty after a fresh sync against the local devnet
  [docker-compose](../docker-compose.yml).
- **Contract `shielded_mints` effect** (Compact: `sendShielded`). Off-chain,
  the wallet matches the effect with a coin commitment in a `ZswapOffer`
  output. See [`intents-and-zswap-mechanics.md`](intents-and-zswap-mechanics.md).
- **Receiving a shielded transfer** (someone else sent you coins via
  `transfer_shielded` against your shielded address).

There is no SDK-level operation to convert unshielded NIGHT into a shielded
representation. Anyone wanting to do so today must deploy or use a contract
that bridges the two ledgers.

## The zero token id

`HashOutput([0u8; 32])` is a load-bearing pitfall because the same 32 bytes
mean different things across the two ledgers:

| As a... | Means |
|---|---|
| `UnshieldedTokenType` | **NIGHT** — the chain's native asset |
| `ShieldedTokenType` | Just "the default shielded token id" — on dev presets, a pre-allocated test token; on mainnet, an artifact of whichever contract or genesis allocation chose to use that id |

Wallet UIs or test code that labels `WalletBalance::shielded.coins[i]` as
"NIGHT" when its `token_type == "00…00"` is **wrong**. Treat shielded coin ids
as opaque unless the deployment explicitly documents an ad-hoc mapping.

## How `WalletBalance` exposes assets

```rust
let balance = provider.balance().await?;

// Fee token
balance.dust.balance_speck;       // u128 — total SPECK across spendable dust UTXOs
balance.dust.spendable_utxos;     // usize

// Unshielded: token_type == "00…00" is NIGHT
for utxo in &balance.unshielded {
    utxo.token_type;              // 64-char hex
    utxo.value;                   // u128 — for NIGHT, in STAR
}

// Shielded: token_type is opaque (do NOT assume "00…00" means NIGHT)
for coin in &balance.shielded.coins {
    coin.token_type;              // 64-char hex
    coin.value;                   // u128 — atomic units of *that* shielded token
}
```

## Where the SDK is NIGHT-aware

Only `MidnightProvider::register_dust` (and the helpers it composes) is
intrinsically NIGHT-specific — dust generation only flows from NIGHT holdings,
so registration filters unshielded UTXOs by the NIGHT token type. Every other
transfer path (`transfer_shielded`, `transfer_unshielded`) is generic over
its token type.

## See also

- [`dust-and-fees.md`](dust-and-fees.md) — DUST mechanics, registration, fee
  balancing, sync phases
- [`intents-and-zswap-mechanics.md`](intents-and-zswap-mechanics.md) — how
  `ZswapOffer`s and contract `*_mints` / `claimed_*` effects compose into a
  transaction
- [`wallet.md`](wallet.md) — SDK wallet API surface
- Upstream specs:
  [`night.md`](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/night.md),
  [`zswap.md`](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/zswap.md),
  [`dust.md`](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/dust.md),
  [`intents-transactions.md`](https://github.com/midnightntwrk/midnight-ledger/blob/main/spec/intents-transactions.md)
