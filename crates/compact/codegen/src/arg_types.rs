//! Derive the interpreter's argument metadata from a circuit's signature.
//!
//! The funded call path executes a circuit's IR against pre-encoded
//! `Value::AlignedValue` arguments. When the IR destructures one of those
//! arguments with `Expr::Field` (e.g. `recipient.is_left`), the interpreter
//! needs two things the IR body alone does not carry:
//!
//! 1. The declared type of each argument, so it knows how to slice the
//!    `AlignedValue` (provided as `(name, TypeRef)` pairs).
//! 2. The layout of any struct/enum used by those arguments. Nested types in
//!    circuit `arguments` are declared *inline* (with `elements`), so they
//!    are harvested into the struct/enum registry the interpreter is given.
//!
//! Both pieces are derived purely from the parsed `CircuitArgument` list.

use crate::ir::{EnumDef, StructDef, TypeRef};
use crate::types::CircuitArgument;

/// Walk `ty` and append an [`ir::StructDef`](StructDef) / [`ir::EnumDef`](EnumDef)
/// for every inline struct/enum definition it carries.
///
/// Definitions already present (matched by name) in `structs`/`enums` are not
/// duplicated, so this can be called repeatedly across a circuit's arguments
/// and across circuits that share types.
pub fn collect_inline_defs(ty: &TypeRef, structs: &mut Vec<StructDef>, enums: &mut Vec<EnumDef>) {
    match ty {
        TypeRef::Struct { name, elements } => {
            // Recurse first so nested types are registered regardless of
            // whether this struct was already seen.
            for elem in elements {
                collect_inline_defs(&elem.ty, structs, enums);
            }
            if !structs.iter().any(|s| &s.name == name) {
                structs.push(StructDef {
                    name: name.clone(),
                    fields: elements.clone(),
                });
            }
        }
        TypeRef::Enum { name, variants } => {
            if !enums.iter().any(|e| &e.name == name) {
                enums.push(EnumDef {
                    name: name.clone(),
                    variants: variants.clone(),
                });
            }
        }
        TypeRef::Alias { inner, .. } => collect_inline_defs(inner, structs, enums),
        TypeRef::Vector { element, .. } => collect_inline_defs(element, structs, enums),
        TypeRef::Tuple { types } => {
            for t in types {
                collect_inline_defs(t, structs, enums);
            }
        }
        TypeRef::Boolean
        | TypeRef::Field
        | TypeRef::Uint { .. }
        | TypeRef::Bytes { .. }
        | TypeRef::Opaque { .. }
        | TypeRef::Void
        | TypeRef::Contract { .. } => {}
    }
}

/// Build the `(name, TypeRef)` argument-type list for a circuit's arguments.
/// Aliases resolve to their inner type: the interpreter has no alias node.
pub fn circuit_arg_types(arguments: &[CircuitArgument]) -> Vec<(String, TypeRef)> {
    arguments
        .iter()
        .map(|arg| (arg.name.clone(), arg.ty.resolved().clone()))
        .collect()
}

/// Harvest all inline struct/enum definitions referenced by a circuit's
/// arguments, appended to the supplied registries (deduplicated by name).
pub fn collect_argument_defs(
    arguments: &[CircuitArgument],
    structs: &mut Vec<StructDef>,
    enums: &mut Vec<EnumDef>,
) {
    for arg in arguments {
        collect_inline_defs(&arg.ty, structs, enums);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recipient argument of the mint circuit: an `Either` whose `left` is
    /// a `ZswapCoinPublicKey` and `right` a `ContractAddress`, both declared
    /// inline. This is the exact shape the interpreter must destructure.
    fn either_recipient_arg_json() -> &'static str {
        r#"{
            "name": "recipient",
            "type": {
                "type-name": "Struct",
                "name": "Either",
                "elements": [
                    { "name": "is_left", "type": { "type-name": "Boolean" } },
                    {
                        "name": "left",
                        "type": {
                            "type-name": "Struct",
                            "name": "ZswapCoinPublicKey",
                            "elements": [
                                { "name": "bytes", "type": { "type-name": "Bytes", "length": 32 } }
                            ]
                        }
                    },
                    {
                        "name": "right",
                        "type": {
                            "type-name": "Struct",
                            "name": "ContractAddress",
                            "elements": [
                                { "name": "bytes", "type": { "type-name": "Bytes", "length": 32 } }
                            ]
                        }
                    }
                ]
            }
        }"#
    }

    fn parse_arg(json: &str) -> CircuitArgument {
        serde_json::from_str(json).expect("parse CircuitArgument")
    }

    #[test]
    fn wide_uint_bounds_deserialize_from_numbers_and_strings() {
        let from_number: TypeRef =
            serde_json::from_str(r#"{ "type-name": "Uint", "maxval": 36893488147419103230 }"#)
                .expect("number maxval");
        let from_string: TypeRef =
            serde_json::from_str(r#"{ "type-name": "Uint", "maxval": "36893488147419103230" }"#)
                .expect("string maxval");
        for t in [from_number, from_string] {
            match t {
                TypeRef::Uint { maxval } => assert_eq!(maxval, "36893488147419103230"),
                other => panic!("expected Uint, got {other:?}"),
            }
        }
    }

    #[test]
    fn aliases_resolve_transparently_for_the_interpreter() {
        let arg: CircuitArgument = serde_json::from_str(
            r#"{ "name": "n", "type": { "type-name": "Alias", "name": "JobId",
                 "type": { "type-name": "Uint", "maxval": "255" } } }"#,
        )
        .expect("alias arg");
        let arg_types = circuit_arg_types(std::slice::from_ref(&arg));
        assert!(matches!(&arg_types[0].1, TypeRef::Uint { maxval } if maxval == "255"));
    }

    #[test]
    fn collect_inline_defs_harvests_nested_structs() {
        let arg = parse_arg(either_recipient_arg_json());
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        collect_inline_defs(&arg.ty, &mut structs, &mut enums);

        let names: Vec<&str> = structs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Either"), "missing Either: {names:?}");
        assert!(
            names.contains(&"ZswapCoinPublicKey"),
            "missing ZswapCoinPublicKey: {names:?}"
        );
        assert!(
            names.contains(&"ContractAddress"),
            "missing ContractAddress: {names:?}"
        );

        // The Either struct's fields preserve order and types.
        let either = structs.iter().find(|s| s.name == "Either").unwrap();
        let field_names: Vec<&str> = either.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(field_names, ["is_left", "left", "right"]);
        assert!(matches!(either.fields[0].ty, TypeRef::Boolean));
        assert!(matches!(
            &either.fields[1].ty,
            TypeRef::Struct { name, .. } if name == "ZswapCoinPublicKey"
        ));
    }

    #[test]
    fn collect_inline_defs_deduplicates_by_name() {
        let arg = parse_arg(either_recipient_arg_json());
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        collect_inline_defs(&arg.ty, &mut structs, &mut enums);
        let count_before = structs.len();
        // Harvesting the same argument again must not add duplicates.
        collect_inline_defs(&arg.ty, &mut structs, &mut enums);
        assert_eq!(structs.len(), count_before);
    }

    #[test]
    fn circuit_arg_types_pairs_names_with_type_refs() {
        let arg = parse_arg(either_recipient_arg_json());
        let arg_types = circuit_arg_types(std::slice::from_ref(&arg));
        assert_eq!(arg_types.len(), 1);
        assert_eq!(arg_types[0].0, "recipient");
        assert!(matches!(
            &arg_types[0].1,
            TypeRef::Struct { name, .. } if name == "Either"
        ));
    }
}
