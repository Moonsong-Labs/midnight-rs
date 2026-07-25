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

/// A fixed-size vector of `N` values of type `T`: the Rust image of Compact's
/// `Vector<N, T>`.
///
/// It wraps `[T; N]` for the same reason [`Bytes<N>`] wraps `[u8; N]`: the FAB
/// traits (`Aligned`, `TryFrom<&ValueSlice>`) that a generated ledger struct
/// needs on each field cannot be implemented on a bare `[T; N]`, because both
/// the array and the traits are foreign here, so the orphan rule forbids it.
/// Upstream provides those impls only for `[u8; N]`, which is why `Vector<N, u8>`
/// would compile but `Vector<N, SomeStruct>` did not. Wrapping the array in a
/// type we own lets us carry the impls generically over any element type.
///
/// `Deref`, `From<[T; N]>`, and `IntoIterator` keep it about as ergonomic as the
/// bare array at the call sites that build or read one.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Vector<const N: usize, T>(pub [T; N]);

impl<const N: usize, T> Vector<N, T> {
    /// Unwraps into the inner `[T; N]`.
    pub fn into_inner(self) -> [T; N] {
        self.0
    }
}

impl<const N: usize, T> From<[T; N]> for Vector<N, T> {
    fn from(inner: [T; N]) -> Self {
        Self(inner)
    }
}

impl<const N: usize, T> From<Vector<N, T>> for [T; N] {
    fn from(v: Vector<N, T>) -> [T; N] {
        v.0
    }
}

impl<const N: usize, T> std::ops::Deref for Vector<N, T> {
    type Target = [T; N];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const N: usize, T> IntoIterator for Vector<N, T> {
    type Item = T;
    type IntoIter = std::array::IntoIter<T, N>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<const N: usize, T: std::fmt::Debug> std::fmt::Debug for Vector<N, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// The alignment of a fixed vector is the element alignment repeated `N` times,
// which is exactly what a struct whose fields are all `T` would produce, so the
// bytes line up with the on-chain encoding.
impl<const N: usize, T: Aligned> Aligned for Vector<N, T> {
    fn alignment() -> Alignment {
        let element = <T as Aligned>::alignment();
        Alignment::concat(std::iter::repeat_n(&element, N))
    }
}

impl<const N: usize, T> From<Vector<N, T>> for Value
where
    Value: From<T>,
{
    fn from(v: Vector<N, T>) -> Value {
        let parts: Vec<Value> = v.0.into_iter().map(Value::from).collect();
        Value::concat(parts.iter())
    }
}

impl<const N: usize, T> TryFrom<&ValueSlice> for Vector<N, T>
where
    T: Aligned + for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
{
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        let err = || InvalidBuiltinDecode(std::any::type_name::<Vector<N, T>>());
        let element = <T as Aligned>::alignment();
        let mut rest = value;
        let mut out: Vec<T> = Vec::with_capacity(N);
        for _ in 0..N {
            // Peel off one element's worth of atoms and decode it, leaving the
            // remainder for the next iteration.
            let (chunk, tail) = element.consume(rest).ok_or_else(err)?;
            out.push(T::try_from(&chunk)?);
            rest = tail;
        }
        if !rest.0.is_empty() {
            return Err(err());
        }
        out.try_into().map(Self).map_err(|_| err())
    }
}

#[cfg(test)]
mod tests {
    use midnight_base_crypto::fab::{Aligned, AlignedValue, Alignment, AlignmentAtom};

    /// Generated code encodes a Compact `Opaque` argument as
    /// `AlignedValue::from(Vec<u8>)`. That has to stay equivalent to the Compact
    /// runtime's `CompactTypeOpaqueString` / `CompactTypeOpaqueUint8Array`,
    /// which are a single `Compress`-aligned atom holding the bytes verbatim
    /// (`tools/compact-compiler/runtime/src/compact-types.ts`). If this breaks,
    /// opaque circuit arguments silently reach the chain wrong.
    #[test]
    fn opaque_bytes_encode_as_one_compress_atom() {
        let bytes = b"hello".to_vec();
        let encoded = AlignedValue::from(bytes.clone());

        assert_eq!(
            encoded.alignment,
            Alignment::singleton(AlignmentAtom::Compress),
            "opaque values must carry a single Compress alignment atom"
        );
        assert_eq!(
            <Vec<u8> as Aligned>::alignment(),
            Alignment::singleton(AlignmentAtom::Compress),
            "the Rust type generated for opaque arguments must align the same way"
        );

        let atoms = &encoded.value.0;
        assert_eq!(atoms.len(), 1, "expected exactly one value atom");
        assert_eq!(atoms[0].0, bytes, "the atom must hold the bytes verbatim");
    }

    use super::{Bytes, Vector};

    /// A `Vector<N, T>` encodes as the flat concatenation of its `N` elements,
    /// each at its own alignment, which is what the on-chain `Vector<N, T>`
    /// descriptor produces. Uses `Bytes<32>` as a single-atom element.
    #[test]
    fn vector_alignment_and_encoding_is_flat_concat() {
        let v: Vector<3, Bytes<32>> =
            Vector([Bytes([1u8; 32]), Bytes([2u8; 32]), Bytes([3u8; 32])]);

        assert_eq!(
            <Vector<3, Bytes<32>> as Aligned>::alignment(),
            {
                let a = <Bytes<32> as Aligned>::alignment();
                Alignment::concat(std::iter::repeat_n(&a, 3))
            },
            "alignment is the element alignment repeated N times"
        );

        let av = AlignedValue::from(v);
        assert_eq!(av.value.0.len(), 3, "one atom per element, in order");
        assert_eq!(av.value.0[0].0, vec![1u8; 32]);
        assert_eq!(av.value.0[2].0, vec![3u8; 32]);
    }

    /// A vector of a multi-atom element (a `(Uint, Bytes<32>)` pair here, the
    /// shape of a struct field) must split back into exactly its elements on
    /// decode. This is the case a bare `[T; N]` could not support.
    #[test]
    fn vector_of_multi_atom_elements_round_trips() {
        // Two atoms per element: a u32 and a Bytes<32>.
        type Pair = (u32, Bytes<32>);
        let v: Vector<3, Pair> = Vector([
            (10, Bytes([0xAA; 32])),
            (20, Bytes([0xBB; 32])),
            (30, Bytes([0xCC; 32])),
        ]);

        let av = AlignedValue::from(v.clone());
        assert_eq!(av.value.0.len(), 6, "3 elements * 2 atoms each");

        let decoded = Vector::<3, Pair>::try_from(&*av.value).expect("decode round-trips");
        assert_eq!(decoded, v, "encode then decode is the identity");
    }

    /// A slice whose atom count is not a multiple of the element width, or has
    /// leftover atoms, must be rejected rather than silently truncated.
    #[test]
    fn vector_decode_rejects_a_mis_sized_slice() {
        let two_atoms =
            AlignedValue::from(Vector::<2, Bytes<32>>([Bytes([0; 32]), Bytes([0; 32])]));
        // Ask for three elements from a two-element encoding.
        assert!(Vector::<3, Bytes<32>>::try_from(&*two_atoms.value).is_err());

        let three_atoms = AlignedValue::from(Vector::<3, Bytes<32>>([
            Bytes([0; 32]),
            Bytes([0; 32]),
            Bytes([0; 32]),
        ]));
        // Ask for two: one element's worth of atoms is left over.
        assert!(Vector::<2, Bytes<32>>::try_from(&*three_atoms.value).is_err());
    }
}
