//! A contract with secp256r1 ledger fields, circuit arguments, a struct of
//! secp256r1 fields and a secp256r1 witness binds each type to its
//! `compact_runtime` value, and a ledger built from those values reads
//! them back through its typed accessors.

use compact_bindgen::midnight_contract::runtime::{Secp256r1Base, Secp256r1Point, Secp256r1Scalar};

compact_bindgen::contract!(
    "../../../../tests/conformance/fixtures/secp256r1/compiler/analyzed-ir.sexp"
);

fn main() {
    let mut two = [0u8; 32];
    two[31] = 2;
    let base = Secp256r1Base::from_be_bytes(two).unwrap();
    let scalar = Secp256r1Scalar::from_be_bytes(two).unwrap();
    let point = Secp256r1Point::default();

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

    let _signature = Secp256r1EcdsaSignature {
        r: scalar,
        s: scalar,
    };
}
