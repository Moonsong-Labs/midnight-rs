//! Derive the interpreter's argument metadata from a circuit's signature.
//!
//! The funded call path executes a circuit's IR against pre-encoded
//! `Value::AlignedValue` arguments. When the IR destructures one of those
//! arguments with `Expr::Field` (e.g. `recipient.is_left`), the interpreter
//! needs two things the IR body alone does not carry:
//!
//! 1. The declared type of each argument, so it knows how to slice the
//!    `AlignedValue` (provided as `(name, Type)` pairs).
//! 2. The layout of any struct/enum used by those arguments. Nested types in
//!    circuit `arguments` are declared *inline* (with their fields), so they
//!    are harvested into the struct/enum registry the interpreter is given.
//!
//! Both pieces are derived purely from the parsed `CircuitArgument` list.

use crate::ir::{EnumDef, StructDef};
use crate::nir::Type;
use crate::types::CircuitArgument;

/// Walk `ty` and append an [`ir::StructDef`](StructDef) / [`ir::EnumDef`](EnumDef)
/// for every inline struct/enum definition it carries.
///
/// Definitions already present (matched by name) in `structs`/`enums` are not
/// duplicated, so this can be called repeatedly across a circuit's arguments
/// and across circuits that share types.
pub fn collect_inline_defs(ty: &Type, structs: &mut Vec<StructDef>, enums: &mut Vec<EnumDef>) {
    match ty {
        Type::Struct { name, fields } => {
            // Recurse first so nested types are registered regardless of
            // whether this struct was already seen.
            for (_, field_ty) in fields {
                collect_inline_defs(field_ty, structs, enums);
            }
            if !structs.iter().any(|s| &s.name == name) {
                structs.push(StructDef {
                    name: name.clone(),
                    fields: fields.clone(),
                });
            }
        }
        Type::Enum { name, variants } => {
            if !enums.iter().any(|e| &e.name == name) {
                enums.push(EnumDef {
                    name: name.clone(),
                    variants: variants.clone(),
                });
            }
        }
        Type::Alias { ty: inner, .. } | Type::Vector { ty: inner, .. } => {
            collect_inline_defs(inner, structs, enums);
        }
        Type::Tuple(types) => {
            for t in types {
                collect_inline_defs(t, structs, enums);
            }
        }
        Type::Boolean
        | Type::Field(_)
        | Type::Unsigned(_)
        | Type::Point(_)
        | Type::Bytes(_)
        | Type::Opaque(_)
        | Type::Contract { .. } => {}
        // Rejected at load by `normalized::check_type`; unreachable here.
        Type::Adt { .. } | Type::TypeVar(_) | Type::Unknown => {}
    }
}

/// Build the `(name, Type)` argument-type list for a circuit's arguments.
/// Aliases resolve to their inner type: the interpreter has no alias node.
pub fn circuit_arg_types(arguments: &[CircuitArgument]) -> Vec<(String, Type)> {
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
    fn either_recipient_arg() -> CircuitArgument {
        // A 32-byte wrapper struct, the shape of both Either branches.
        let named = |n: &str| Type::Struct {
            name: n.to_string(),
            fields: vec![("bytes".to_string(), Type::Bytes(32))],
        };
        CircuitArgument {
            name: "recipient".to_string(),
            ty: Type::Struct {
                name: "Either".to_string(),
                fields: vec![
                    ("is_left".to_string(), Type::Boolean),
                    ("left".to_string(), named("ZswapCoinPublicKey")),
                    ("right".to_string(), named("ContractAddress")),
                ],
            },
        }
    }

    #[test]
    fn aliases_resolve_transparently_for_the_interpreter() {
        let arg = CircuitArgument {
            name: "n".to_string(),
            ty: Type::Alias {
                nominal: true,
                name: "JobId".to_string(),
                ty: Box::new(Type::Unsigned(255u32.into())),
            },
        };
        let arg_types = circuit_arg_types(std::slice::from_ref(&arg));
        assert!(matches!(&arg_types[0].1, Type::Unsigned(maxval) if *maxval == 255u32.into()));
    }

    #[test]
    fn collect_inline_defs_harvests_nested_structs() {
        let arg = either_recipient_arg();
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
        let field_names: Vec<&str> = either.fields.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(field_names, ["is_left", "left", "right"]);
        assert!(matches!(either.fields[0].1, Type::Boolean));
        assert!(matches!(
            &either.fields[1].1,
            Type::Struct { name, .. } if name == "ZswapCoinPublicKey"
        ));
    }

    #[test]
    fn collect_inline_defs_deduplicates_by_name() {
        let arg = either_recipient_arg();
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
        let arg = either_recipient_arg();
        let arg_types = circuit_arg_types(std::slice::from_ref(&arg));
        assert_eq!(arg_types.len(), 1);
        assert_eq!(arg_types[0].0, "recipient");
        assert!(matches!(
            &arg_types[0].1,
            Type::Struct { name, .. } if name == "Either"
        ));
    }
}
