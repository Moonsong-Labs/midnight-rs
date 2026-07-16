//! Additional types and conversions bridging gaps in midnight-ledger's
//! `TryFrom<&ValueSlice>` coverage for use in generated tuple decomposition.

use midnight_base_crypto::fab::{
    Aligned, Alignment, InvalidBuiltinDecode, Value, ValueAtom, ValueSlice,
};

/// A fixed-size byte array newtype that implements `TryFrom<&ValueSlice>`.
///
/// Midnight-ledger provides `TryFrom<ValueAtom> for [u8; N]` and
/// `Aligned for [u8; N]`, but not `TryFrom<&ValueSlice> for [u8; N]`.
/// This wrapper fills that gap so that `Bytes<N>` can participate in
/// tuple decomposition from `ValueSlice`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Bytes<const N: usize>(pub [u8; N]);

impl<const N: usize> Bytes<N> {
    /// Unwraps into the inner `[u8; N]`.
    pub fn into_inner(self) -> [u8; N] {
        self.0
    }
}

impl<const N: usize> AsRef<[u8; N]> for Bytes<N> {
    fn as_ref(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> AsRef<[u8]> for Bytes<N> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> From<[u8; N]> for Bytes<N> {
    fn from(arr: [u8; N]) -> Self {
        Self(arr)
    }
}

impl<const N: usize> From<Bytes<N>> for [u8; N] {
    fn from(b: Bytes<N>) -> [u8; N] {
        b.0
    }
}

impl<const N: usize> std::ops::Deref for Bytes<N> {
    type Target = [u8; N];
    fn deref(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> std::fmt::Debug for Bytes<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bytes(0x{})", hex::encode(self.0))
    }
}

impl<const N: usize> std::fmt::Display for Bytes<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

// --- Aligned ---

impl<const N: usize> Aligned for Bytes<N> {
    fn alignment() -> Alignment {
        <[u8; N] as Aligned>::alignment()
    }
}

// --- Value conversions ---

impl<const N: usize> From<Bytes<N>> for Value {
    fn from(b: Bytes<N>) -> Value {
        Value::from(b.0)
    }
}

impl<const N: usize> TryFrom<&ValueSlice> for Bytes<N> {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        if value.0.len() == 1 {
            let arr: [u8; N] = value.0[0].clone().try_into()?;
            Ok(Self(arr))
        } else {
            Err(InvalidBuiltinDecode(std::any::type_name::<[u8; N]>()))
        }
    }
}

// Into<AlignedValue> for key lookups in MapAccessor/SetAccessor
impl<const N: usize> From<Bytes<N>> for ValueAtom {
    fn from(b: Bytes<N>) -> ValueAtom {
        b.0.into()
    }
}
