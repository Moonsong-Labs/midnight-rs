//! The generic structs of Compact's standard library: [`Maybe`] and
//! [`Either`].
//!
//! Generated bindings use these types for every instantiation. One generated
//! struct per Compact struct name cannot hold two instantiations of it.

use midnight_base_crypto::fab::{
    Aligned, AlignedValue, Alignment, InvalidBuiltinDecode, ValueSlice,
};

/// Compact's `Maybe<T>`: a `value` that is present when `is_some` is true.
///
/// A `Maybe` that is not set still holds a `value`, the default of `T`,
/// because the encoding has a fixed width.
///
/// # Examples
///
/// ```
/// use midnight_typed_state::Maybe;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
