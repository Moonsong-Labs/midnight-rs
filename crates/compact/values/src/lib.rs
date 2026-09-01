//! The value types generated Compact bindings name.
//!
//! A circuit taking Compact's `Field` or `JubjubPoint` needs a Rust type in
//! its signature. Naming the ledger's own field element there would pin the
//! binding, and every caller of it, to one ledger generation.
//!
//! These types carry the same encoding instead. `midnight-base-crypto` holds
//! the field-aligned binary format and is the same crate in every generation,
//! so a value encoded here is the value the ledger reads. `Aligned` plus
//! `Into<Value>` is all an `AlignedValue` needs: base-crypto converts from
//! there.

use midnight_base_crypto::fab::{
    Aligned, Alignment, AlignmentAtom, AlignmentSegment, InvalidBuiltinDecode, Value, ValueAtom,
    ValueSlice,
};

/// A field element, as little-endian bytes.
///
/// Convert with the `From` impls on the ledger's own field type, which every
/// generation provides for this one.
#[derive(Debug, Clone, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct Field(Vec<u8>);

impl Field {
    /// The element these little-endian bytes encode.
    ///
    /// Trailing zero bytes are dropped, because the encoded form is
    /// normalized and two values that encode the same must compare equal.
    pub fn from_le_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        let mut bytes = bytes.into();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Self(bytes)
    }

    /// This element's little-endian bytes.
    pub fn as_le_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<u64> for Field {
    fn from(value: u64) -> Self {
        Self::from_le_bytes(value.to_le_bytes())
    }
}

impl Aligned for Field {
    fn alignment() -> Alignment {
        Alignment::singleton(AlignmentAtom::Field)
    }
}

impl From<Field> for Value {
    fn from(field: Field) -> Value {
        Value(vec![ValueAtom(field.0).normalize()])
    }
}

/// A point on the embedded curve, as its two coordinates.
///
/// The identity has no coordinates, and encodes as a pair of zeros, which is
/// what the ledger's own point type does.
#[derive(Debug, Clone, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
pub struct JubjubPoint {
    /// The x coordinate, zero for the identity.
    pub x: Field,
    /// The y coordinate, zero for the identity.
    pub y: Field,
}

impl Aligned for JubjubPoint {
    fn alignment() -> Alignment {
        Alignment(vec![
            AlignmentSegment::Atom(AlignmentAtom::Field),
            AlignmentSegment::Atom(AlignmentAtom::Field),
        ])
    }
}

impl From<JubjubPoint> for Value {
    fn from(point: JubjubPoint) -> Value {
        Value(vec![
            ValueAtom(point.x.0).normalize(),
            ValueAtom(point.y.0).normalize(),
        ])
    }
}

impl TryFrom<&ValueAtom> for Field {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueAtom) -> Result<Field, InvalidBuiltinDecode> {
        Ok(Field::from_le_bytes(value.0.clone()))
    }
}

impl TryFrom<&ValueSlice> for Field {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Field, InvalidBuiltinDecode> {
        match value.0.len() {
            1 => Field::try_from(&value.0[0]),
            _ => Err(InvalidBuiltinDecode("Field")),
        }
    }
}

impl TryFrom<&ValueSlice> for JubjubPoint {
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<JubjubPoint, InvalidBuiltinDecode> {
        match value.0.len() {
            2 => Ok(JubjubPoint {
                x: Field::try_from(&value.0[0])?,
                y: Field::try_from(&value.0[1])?,
            }),
            _ => Err(InvalidBuiltinDecode("JubjubPoint")),
        }
    }
}
