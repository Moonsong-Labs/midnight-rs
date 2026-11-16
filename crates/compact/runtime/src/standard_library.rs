//! Types of Compact's standard library: the generic structs [`Maybe`] and
//! [`Either`], and the event [`Misc`].
//!
//! Generated bindings use `Maybe` and `Either` for every instantiation. One
//! generated struct per Compact struct name cannot hold two instantiations of
//! it.

use midnight_onchain_runtime::ops::{LogEventType, VersionedLogItem};
use midnight_typed_state::{
    Aligned, AlignedValue, Alignment, Bytes, InMemoryDB, InvalidBuiltinDecode, ValueSlice,
    cell_value,
};

/// Compact's `Maybe<T>`: a `value` that is present when `is_some` is true.
///
/// A `Maybe` that is not set still holds a `value`, the default of `T`,
/// because the encoding has a fixed width.
///
/// # Examples
///
/// ```
/// use compact_runtime::Maybe;
///
/// let some = Maybe { is_some: true, value: 7u8 };
/// assert_eq!(some.into_option(), Some(7));
/// assert_eq!(Maybe::<u8>::default().into_option(), None);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Maybe<T> {
    pub is_some: bool,
    pub value: T,
}

impl<T> Maybe<T> {
    /// `Some(value)` when `is_some` is true, and `None` otherwise.
    pub fn into_option(self) -> Option<T> {
        self.is_some.then_some(self.value)
    }
}

/// Compact's `Either<A, B>`: `left` when `is_left` is true, and `right`
/// otherwise.
///
/// Both sides always hold a value, because the encoding has a fixed width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Either<A, B> {
    pub is_left: bool,
    pub left: A,
    pub right: B,
}

impl<T: Aligned> Aligned for Maybe<T> {
    fn alignment() -> Alignment {
        Alignment::concat([&bool::alignment(), &T::alignment()])
    }
}

impl<A: Aligned, B: Aligned> Aligned for Either<A, B> {
    fn alignment() -> Alignment {
        Alignment::concat([&bool::alignment(), &A::alignment(), &B::alignment()])
    }
}

// The bound is on `AlignedValue`, not on `Value`: a generated struct or enum
// converts into `AlignedValue` only.
impl<T> From<Maybe<T>> for AlignedValue
where
    AlignedValue: From<T>,
{
    fn from(maybe: Maybe<T>) -> AlignedValue {
        AlignedValue::concat([
            &<AlignedValue as From<bool>>::from(maybe.is_some),
            &AlignedValue::from(maybe.value),
        ])
    }
}

impl<A, B> From<Either<A, B>> for AlignedValue
where
    AlignedValue: From<A> + From<B>,
{
    fn from(either: Either<A, B>) -> AlignedValue {
        AlignedValue::concat([
            &<AlignedValue as From<bool>>::from(either.is_left),
            &AlignedValue::from(either.left),
            &AlignedValue::from(either.right),
        ])
    }
}

impl<T> TryFrom<&ValueSlice> for Maybe<T>
where
    T: Aligned + for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
{
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        let (is_some, value): (bool, T) = value.try_into()?;
        Ok(Self { is_some, value })
    }
}

impl<A, B> TryFrom<&ValueSlice> for Either<A, B>
where
    A: Aligned + for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
    B: Aligned + for<'a> TryFrom<&'a ValueSlice, Error = InvalidBuiltinDecode>,
{
    type Error = InvalidBuiltinDecode;

    fn try_from(value: &ValueSlice) -> Result<Self, InvalidBuiltinDecode> {
        let (is_left, left, right): (bool, A, B) = value.try_into()?;
        Ok(Self {
            is_left,
            left,
            right,
        })
    }
}

/// The Compact standard event `Misc`, the event for contract-defined data.
///
/// A contract chooses what `name` holds. It usually puts a value that
/// `serialize<T, 256>` made in `payload`, and the contract's own pure circuit
/// (`deserialize<T, 256>`) reads it back. The fields use the bindings'
/// `Bytes<N>`, so `payload` goes to a generated pure circuit as it is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Misc {
    pub name: Bytes<32>,
    pub payload: Bytes<256>,
}

impl Misc {
    /// Decode a log item as a `Misc` event of the layout compactc writes.
    ///
    /// The item must have version 1 and the type `Misc`. Its data must be one
    /// cell under the alignment `bytes(288)`, with one atom of at most 288
    /// bytes. The decode pads the atom with zeros to 288 bytes, because the
    /// ledger trims trailing zeros. `name` is bytes 0..32 and `payload` is
    /// bytes 32..288.
    ///
    /// # Errors
    ///
    /// - [`DecodeLogError::Version`] for a version other than 1. Version 0 is
    ///   the ledger's form for a logged value that is not an event.
    /// - [`DecodeLogError::EventType`] for another event type.
    /// - [`DecodeLogError::Data`] for any other data.
    pub fn decode_log_data(item: &VersionedLogItem<InMemoryDB>) -> Result<Misc, DecodeLogError> {
        if item.version != 1 {
            return Err(DecodeLogError::Version(item.version));
        }
        if item.event_type != LogEventType::Misc {
            return Err(DecodeLogError::EventType(item.event_type));
        }
        let cell = cell_value(&item.data).map_err(|_| DecodeLogError::Data)?;
        if cell.alignment != Bytes::<288>::alignment() {
            return Err(DecodeLogError::Data);
        }
        let Bytes(data) = Bytes::<288>::try_from(&*cell.value).map_err(|_| DecodeLogError::Data)?;
        let mut name = [0; 32];
        let mut payload = [0; 256];
        name.copy_from_slice(&data[..32]);
        payload.copy_from_slice(&data[32..]);
        Ok(Misc {
            name: Bytes(name),
            payload: Bytes(payload),
        })
    }
}

/// Why a log item is not the event that the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeLogError {
    #[error("event version {0} is not 1")]
    Version(u32),
    #[error("the log is a {0:?} event")]
    EventType(LogEventType),
    #[error("the event data is not one cell of the event's width")]
    Data,
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_typed_state::{StateValue, cell_as};

    // Distinct values in each field, so a swap of two fields shows.

    #[test]
    fn maybe_lays_out_is_some_then_value() {
        let maybe = Maybe {
            is_some: true,
            value: 9u16,
        };
        let expected = AlignedValue::concat([&AlignedValue::from(true), &AlignedValue::from(9u16)]);
        assert_eq!(Maybe::<u16>::alignment(), expected.alignment);
        assert_eq!(AlignedValue::from(maybe), expected);
        assert_eq!(Maybe::<u16>::try_from(&*expected.value), Ok(maybe));
    }

    #[test]
    fn either_lays_out_is_left_then_left_then_right() {
        let either = Either {
            is_left: false,
            left: 1u8,
            right: 2u16,
        };
        let expected = AlignedValue::concat([
            &AlignedValue::from(false),
            &AlignedValue::from(1u8),
            &AlignedValue::from(2u16),
        ]);
        assert_eq!(Either::<u8, u16>::alignment(), expected.alignment);
        assert_eq!(AlignedValue::from(either), expected);
        assert_eq!(Either::<u8, u16>::try_from(&*expected.value), Ok(either));
    }

    fn misc_item<const N: usize>(version: u32) -> VersionedLogItem<InMemoryDB> {
        let mut data = [0; N];
        data[..32].copy_from_slice(&[0x07; 32]);
        data[32..43].copy_from_slice(b"midnight-rs");
        VersionedLogItem {
            version,
            event_type: LogEventType::Misc,
            data: StateValue::from(AlignedValue::from(Bytes(data))),
        }
    }

    #[test]
    fn misc_refuses_version_2() {
        assert_eq!(
            Misc::decode_log_data(&misc_item::<288>(2)),
            Err(DecodeLogError::Version(2))
        );
    }

    #[test]
    fn misc_refuses_a_cell_of_another_width() {
        let item = misc_item::<287>(1);
        assert!(
            cell_as::<Bytes<288>>(&item.data).is_ok(),
            "the bytes(287) cell must pass every check but the width"
        );
        assert_eq!(Misc::decode_log_data(&item), Err(DecodeLogError::Data));
    }
}
