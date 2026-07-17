//! Bridge a circuit's `arguments` metadata (the `type-name` ABI schema used in
//! the top level of `contract-info.json`) into the IR vocabulary the
//! interpreter consumes (`ir::TypeRef`, `ir::StructDef`, `ir::EnumDef`).
//!
//! The funded call path executes a circuit's IR against pre-encoded
//! `Value::AlignedValue` arguments. When the IR destructures one of those
//! arguments with `Expr::Field` (e.g. `recipient.is_left`), the interpreter
//! needs two things the IR body alone does not carry:
//!
//! 1. The declared type of each argument, so it knows how to slice the
//!    `AlignedValue` (provided as `(name, TypeRef)` pairs).
//! 2. The layout of any struct/enum used by those arguments. Nested types in
//!    circuit `arguments` are declared *inline* (with `elements`), not
//!    referenced from the top-level `structs` array, so they must be harvested
//!    into the struct/enum registry the interpreter is given.
//!
//! Both pieces are derived purely from the parsed `CircuitArgument` list.

use crate::ir::{EnumDef, StructDef, StructField, TypeRef};
use crate::types::{CircuitArgument, TypeNode};

/// Convert a contract-info [`TypeNode`] into the IR's [`TypeRef`].
///
/// Aliases are transparent: an `Alias { inner }` resolves to its inner type's
/// `TypeRef`, matching how the interpreter treats them (it has no alias node).
pub fn type_node_to_type_ref(node: &TypeNode) -> TypeRef {
    match node {
        TypeNode::Boolean => TypeRef::Boolean,
        TypeNode::Field => TypeRef::Field,
        TypeNode::Uint { maxval } => TypeRef::Uint {
            maxval: json_number_to_string(maxval),
        },
        TypeNode::Bytes { length } => TypeRef::Bytes { length: *length },
        TypeNode::Vector { length, inner } => TypeRef::Vector {
            length: *length,
            element: Box::new(type_node_to_type_ref(inner)),
        },
        TypeNode::Tuple { types } => TypeRef::Tuple {
            types: types.iter().map(type_node_to_type_ref).collect(),
        },
        TypeNode::Struct { name, .. } => TypeRef::Struct { name: name.clone() },
        TypeNode::Enum { name, .. } => TypeRef::Enum { name: name.clone() },
        TypeNode::Alias { inner, .. } => type_node_to_type_ref(inner),
        TypeNode::Opaque { ts_type } => TypeRef::Opaque {
            name: ts_type.clone().unwrap_or_default(),
        },
        // A contract handle and an unknown future type never participate in
        // argument field-slicing; map them to an opaque marker rather than
        // panicking so unrelated circuits in the same contract still expand.
        TypeNode::Contract { name } => TypeRef::Opaque {
            name: name.clone().unwrap_or_else(|| "Contract".to_string()),
        },
        TypeNode::Unknown { type_name } => TypeRef::Opaque {
            name: type_name.clone(),
        },
    }
}

/// Walk `node` and append an [`ir::StructDef`](StructDef) / [`ir::EnumDef`](EnumDef)
/// for every inline struct/enum definition it carries.
///
/// Definitions already present (matched by name) in `structs`/`enums` are not
/// duplicated, so this can be called repeatedly across a circuit's arguments
/// and across circuits that share types.
pub fn collect_inline_defs(
    node: &TypeNode,
    structs: &mut Vec<StructDef>,
    enums: &mut Vec<EnumDef>,
) {
    match node {
        TypeNode::Struct { name, elements } => {
            // Recurse first so nested types are registered regardless of
            // whether this struct was already seen.
            for elem in elements {
                collect_inline_defs(&elem.type_node, structs, enums);
            }
            if !structs.iter().any(|s| &s.name == name) {
                structs.push(StructDef {
                    name: name.clone(),
                    fields: elements
                        .iter()
                        .map(|e| StructField {
                            name: e.name.clone(),
                            ty: type_node_to_type_ref(&e.type_node),
                        })
                        .collect(),
                });
            }
        }
        TypeNode::Enum { name, elements } => {
            if !enums.iter().any(|e| &e.name == name) {
                enums.push(EnumDef {
                    name: name.clone(),
                    variants: elements.clone(),
                });
            }
        }
        TypeNode::Alias { inner, .. } | TypeNode::Vector { inner, .. } => {
            collect_inline_defs(inner, structs, enums);
        }
        TypeNode::Tuple { types } => {
            for t in types {
                collect_inline_defs(t, structs, enums);
            }
        }
        TypeNode::Boolean
        | TypeNode::Field
        | TypeNode::Uint { .. }
        | TypeNode::Bytes { .. }
        | TypeNode::Opaque { .. }
        | TypeNode::Contract { .. }
        | TypeNode::Unknown { .. } => {}
    }
}

/// Build the `(name, TypeRef)` argument-type list for a circuit's arguments.
pub fn circuit_arg_types(arguments: &[CircuitArgument]) -> Vec<(String, TypeRef)> {
    arguments
        .iter()
        .map(|arg| (arg.name.clone(), type_node_to_type_ref(&arg.type_node)))
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
        collect_inline_defs(&arg.type_node, structs, enums);
    }
}

/// Render a JSON number (or numeric string) as the plain decimal string the
/// IR's `Uint { maxval }` expects. `serde_json::Value::to_string` renders a
/// number without quotes and a string with them, so strip any quoting.
fn json_number_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
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
    fn primitive_type_nodes_map_to_type_refs() {
        assert!(matches!(
            type_node_to_type_ref(&TypeNode::Boolean),
            TypeRef::Boolean
        ));
        assert!(matches!(
            type_node_to_type_ref(&TypeNode::Bytes { length: 32 }),
            TypeRef::Bytes { length: 32 }
        ));
        match type_node_to_type_ref(&TypeNode::Uint {
            maxval: serde_json::json!(18446744073709551615u64),
        }) {
            TypeRef::Uint { maxval } => assert_eq!(maxval, "18446744073709551615"),
            other => panic!("expected Uint, got {other:?}"),
        }
    }

    #[test]
    fn struct_type_node_maps_to_named_struct_ref() {
        let arg = parse_arg(either_recipient_arg_json());
        match type_node_to_type_ref(&arg.type_node) {
            TypeRef::Struct { name } => assert_eq!(name, "Either"),
            other => panic!("expected Struct, got {other:?}"),
        }
    }

    #[test]
    fn collect_inline_defs_harvests_nested_structs() {
        let arg = parse_arg(either_recipient_arg_json());
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        collect_inline_defs(&arg.type_node, &mut structs, &mut enums);

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
            TypeRef::Struct { name } if name == "ZswapCoinPublicKey"
        ));
    }

    #[test]
    fn collect_inline_defs_deduplicates_by_name() {
        let arg = parse_arg(either_recipient_arg_json());
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        collect_inline_defs(&arg.type_node, &mut structs, &mut enums);
        let count_before = structs.len();
        // Harvesting the same argument again must not add duplicates.
        collect_inline_defs(&arg.type_node, &mut structs, &mut enums);
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
            TypeRef::Struct { name } if name == "Either"
        ));
    }
}
