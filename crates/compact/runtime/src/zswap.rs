//! ZSwap native primitives: the shielded-coin natives a Compact circuit can
//! invoke (`createZswapInput`/`createZswapOutput`/`ownPublicKey`) and the
//! captured output the call/deploy path turns into a transaction ZSwap offer.
//! The Rust counterpart of Minokawa's `zswap` runtime module.

use crate::value::Value;

/// The Compact "witness" native primitives: the `declare-native-entry witness`
/// entries in the compiler's `midnight-natives.ss`. Unlike the pure circuit
/// natives (handled by [`try_builtin`]), these are effectful, they read the
/// caller's key or capture a coin into the transaction, so the interpreter
/// handles them inline in the `Expr::CallWitness` arm of `eval_expr` rather
/// than dispatching to the witness provider / builtin / helper (which has no
/// entry for them and would error). See `docs/compact-natives.md` for the full
/// native table and our coverage; the match in `eval_expr` is exhaustive, so a
/// new variant added here forces a decision at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessNative {
    /// `ownPublicKey() -> ZswapCoinPublicKey`: the caller's coin public key.
    OwnPublicKey,
    /// `createZswapInput(coin) -> []`: a shielded spend (the input counterpart
    /// of [`WitnessNative::CreateZswapOutput`]).
    CreateZswapInput,
    /// `createZswapOutput(coin, recipient) -> []`: a shielded output, captured
    /// for the call/deploy path to build into the transaction's Zswap offer.
    CreateZswapOutput,
}

impl WitnessNative {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ownPublicKey" => Some(Self::OwnPublicKey),
            "createZswapInput" => Some(Self::CreateZswapInput),
            "createZswapOutput" => Some(Self::CreateZswapOutput),
            _ => None,
        }
    }
}

/// A shielded coin the circuit asked to create on-chain via the
/// `createZswapOutput` kernel native (e.g. through `mintShieldedToken` or
/// `sendShielded`).
///
/// `createZswapOutput(coin, recipient)` records no ledger effect of its own
/// (the mint/spend/receive effects are separate `ledger-query` ops); it marks
/// "attach a Zswap output for this coin here". The interpreter captures the
/// raw arg `Value`s so the call/deploy path can build the corresponding
/// `Output` in the transaction's Zswap offer (optionally with a discovery
/// ciphertext keyed to the recipient's encryption public key).
#[derive(Debug, Clone)]
pub struct CircuitZswapOutput {
    /// The `ShieldedCoinInfo` the circuit constructed (nonce, color/type,
    /// value), as evaluated by the interpreter — a struct-encoded value.
    pub coin: Value,
    /// The `Either<ZswapCoinPublicKey, ContractAddress>` recipient the circuit
    /// passed, as evaluated by the interpreter.
    pub recipient: Value,
}

/// A shielded coin the circuit asked to spend on-chain via the
/// `createZswapInput` kernel native (through `sendShielded` / `mergeCoin` /
/// `sendImmediateShielded`).
///
/// `createZswapInput(coin)` is the spend counterpart of
/// [`CircuitZswapOutput`]: it records no ledger effect of its own (the
/// spend/nullifier effects are separate `ledger-query` ops), it marks "spend
/// this coin here". The coin is always contract-owned (a `sendShielded` spends
/// `coinNullifier(coin, kernel.self())`); when the circuit created it earlier in
/// the same call via `createZswapOutput` to itself (as `receiveShielded` does),
/// the two pair into a Zswap transient. The interpreter captures the coin arg so
/// the call/deploy path can build the corresponding `Input` / `Transient`.
#[derive(Debug, Clone)]
pub struct CircuitZswapInput {
    /// The `QualifiedShieldedCoinInfo` the circuit passed (nonce, color/type,
    /// value, mt_index), as evaluated by the interpreter — a struct-encoded
    /// value. `mt_index` is `0` for a coin that was `upcast` from a plain
    /// `ShieldedCoinInfo` (e.g. via `sendImmediateShielded`), i.e. one not (yet)
    /// in the historical Merkle tree.
    pub coin: Value,
}
