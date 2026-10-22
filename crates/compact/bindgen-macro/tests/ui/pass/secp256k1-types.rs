//! A contract with secp256k1 ledger fields, circuit arguments, a struct of
//! secp256k1 fields and a secp256k1 witness binds each type to its
//! `midnight_typed_state` value, and a ledger built from those values reads
//! them back through its typed accessors.

compact_bindgen::contract!(
    "../../../../tests/conformance/fixtures/secp256k1/compiler/analyzed-ir.sexp"
);

fn main() {
    let mut two = [0u8; 32];
    two[31] = 2;
    let base = compact_bindgen::Secp256k1Base::from_be_bytes(two).unwrap();
    let scalar = compact_bindgen::Secp256k1Scalar::from_be_bytes(two).unwrap();
    let point = compact_bindgen::Secp256k1Point::default();

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

    let _signature = Secp256k1EcdsaSignature {
        r: scalar,
        s: scalar,
    };
}
