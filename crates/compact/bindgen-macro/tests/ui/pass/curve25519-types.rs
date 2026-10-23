//! A contract with Curve25519 ledger fields, circuit arguments, a struct of
//! Curve25519 values and a Curve25519 witness binds each type to its
//! `compact_runtime` value, and a ledger built from those values reads
//! them back through its typed accessors.

use compact_bindgen::midnight_contract::runtime::{
    Curve25519Base, Curve25519Point, Curve25519Scalar,
};

compact_bindgen::contract!(
    "../../../../tests/conformance/fixtures/curve25519/compiler/analyzed-ir.sexp"
);

fn main() {
    let mut two = [0u8; 32];
    two[31] = 2;
    let base = Curve25519Base::from_be_bytes(two).unwrap();
    let scalar = Curve25519Scalar::from_be_bytes(two).unwrap();
    let point = Curve25519Point::default();

    let ledger = LedgerInitialState {
        base_cell: base.into(),
        scalar_cell: scalar.into(),
        point_cell: point.into(),
        ..LedgerInitialState::default()
    }
    .into_ledger();
    assert_eq!(ledger.base_cell().unwrap(), base);
    assert_eq!(ledger.scalar_cell().unwrap(), scalar);
    assert_eq!(ledger.point_cell().unwrap(), point);

    let _signature = Ed25519Signature { r: point, s: scalar };
}
