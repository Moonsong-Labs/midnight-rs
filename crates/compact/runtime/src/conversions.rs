//! `Value` conversions to field scalars, curve points, and integers.
//!
//! Shared by the builtin circuits and the interpreter's arithmetic and
//! encoding paths.

use midnight_typed_state::AlignedValue;

use crate::error::InterpreterError;
use crate::value::Value;

pub fn value_to_fr(v: &Value) -> Option<midnight_transient_crypto::curve::Fr> {
    use midnight_transient_crypto::curve::Fr;
    match v {
        // Exact u128 → Fr conversion (`Scalar::from_u128`); a `u64` cast
        // here would silently drop the high bits of wide integers feeding
        // hashes and EC scalar ops.
        Value::Integer(n) => Some(Fr::from(*n)),
        Value::AlignedValue(av) => Fr::try_from(&*av.value).ok(),
        _ => None,
    }
}

/// Decode a [`Value`] holding a Compact `JubjubPoint` into an
/// `EmbeddedGroupAffine`. The on-chain encoding is two `Field` atoms (the
/// affine `x`/`y` coordinates), matching the
/// `TryFrom<&ValueSlice> for EmbeddedGroupAffine` impl in
/// `midnight-transient-crypto`.
pub fn value_to_embedded_group(
    v: &Value,
) -> Option<midnight_transient_crypto::curve::EmbeddedGroupAffine> {
    use midnight_transient_crypto::curve::EmbeddedGroupAffine;
    match v {
        Value::AlignedValue(av) => EmbeddedGroupAffine::try_from(&*av.value).ok(),
        _ => None,
    }
}

/// Interpret a value as a 32-byte `HashOutput` (e.g. a `Bytes<32>` opening or
/// domain separator). FAB atoms are zero-trimmed, so a shorter atom is
/// right-padded with zeros to 32 bytes.
pub fn value_to_hash_output(
    v: &Value,
) -> Result<midnight_base_crypto::hash::HashOutput, InterpreterError> {
    let av = v.try_to_aligned_value()?;
    let atom = av.value.0.first().ok_or_else(|| {
        InterpreterError::TypeError("expected a 32-byte value, got an empty AlignedValue".into())
    })?;
    if atom.0.len() > 32 {
        return Err(InterpreterError::TypeError(format!(
            "expected a 32-byte value, got a {}-byte atom",
            atom.0.len()
        )));
    }
    let mut bytes = [0u8; 32];
    bytes[..atom.0.len()].copy_from_slice(&atom.0);
    Ok(midnight_base_crypto::hash::HashOutput(bytes))
}

/// Coerce a `Value` into a `u128`, accepting:
/// - `Value::Integer(n)` directly
/// - `Value::Bool(b)` as 0/1
/// - `Value::AlignedValue` containing a single Uint or Field atom whose
///   little-endian byte content fits in `u128`
///
/// Returns `None` if the value isn't a recognized integer-shaped form.
pub fn value_to_u128(val: &Value) -> Option<u128> {
    match val {
        Value::Integer(n) => Some(*n),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        Value::AlignedValue(av) => aligned_atom_to_u128(av),
        _ => None,
    }
}

/// Decode the first atom of an `AlignedValue` as a `u128`. FAB atoms are
/// zero-trimmed *little-endian* bytes (`ValueAtom` conversions in
/// midnight-base-crypto fab/conversions.rs), so the atom [0x2C, 0x01] is 300.
/// The alignment is ignored because the prover already enforces shape.
/// Returns `None` for a zero-atom value or an atom wider than 16 bytes.
pub fn aligned_atom_to_u128(av: &AlignedValue) -> Option<u128> {
    let atom = av.value.0.first()?;
    if atom.0.len() > 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    buf[..atom.0.len()].copy_from_slice(&atom.0);
    Some(u128::from_le_bytes(buf))
}
