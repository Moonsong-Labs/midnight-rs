//! Derive the interpreter's argument metadata from a circuit's signature.
//!
//! The funded call path executes a circuit's body against pre-encoded
//! `Value::AlignedValue` arguments. When the body destructures one of those
//! with a field access (e.g. `recipient.is_left`), the interpreter needs each
//! argument's declared type to slice it. A struct type carries its own field
//! list, so the layout follows from the type and needs no registry.

use crate::nir::{Argument, Type};

/// Build the `(name, Type)` argument-type list for a circuit's arguments.
/// Names are the source-level ones, matching the generated call surface.
/// Aliases resolve to their inner type: the interpreter has no alias node.
pub fn circuit_arg_types(arguments: &[Argument]) -> Vec<(String, Type)> {
    arguments
        .iter()
        .map(|arg| (arg.name.name().to_string(), arg.ty.resolved().clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nir::Ident;

    /// The recipient argument of the mint circuit: an `Either` whose `left` is
    /// a `ZswapCoinPublicKey` and `right` a `ContractAddress`, both declared
    /// inline. This is the exact shape the interpreter must destructure.
    fn either_recipient_arg() -> Argument {
        // A 32-byte wrapper struct, the shape of both Either branches.
        let named = |n: &str| Type::Struct {
            name: n.to_string(),
            fields: vec![("bytes".to_string(), Type::Bytes(32))],
        };
        Argument {
            name: Ident("%recipient.1".to_string()),
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
        let arg = Argument {
            name: Ident("%n".to_string()),
            ty: Type::Alias {
                nominal: true,
                name: "JobId".to_string(),
                ty: Box::new(Type::Unsigned(255u32.into())),
            },
        };
        let arg_types = circuit_arg_types(std::slice::from_ref(&arg));
        assert!(matches!(&arg_types[0].1, Type::Unsigned(maxval) if *maxval == 255u32.into()));
    }

    /// The interpreter slices a struct argument using the layout the type
    /// itself carries, so the nested field lists must survive in declaration
    /// order all the way down.
    #[test]
    fn an_argument_type_carries_its_nested_layout_in_order() {
        let arg = either_recipient_arg();
        let Type::Struct { name, fields } = &arg.ty else {
            panic!("recipient should be a struct type")
        };
        assert_eq!(name, "Either");
        let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(field_names, ["is_left", "left", "right"]);
        assert!(matches!(fields[0].1, Type::Boolean));

        // Each branch carries its own fields, which is what lets the
        // interpreter descend into the live variant without a registry.
        for (branch, expected) in [(1, "ZswapCoinPublicKey"), (2, "ContractAddress")] {
            let Type::Struct {
                name,
                fields: inner,
            } = &fields[branch].1
            else {
                panic!("branch {branch} should be a struct type")
            };
            assert_eq!(name, expected);
            assert_eq!(inner.len(), 1);
            assert!(matches!(inner[0].1, Type::Bytes(32)));
        }
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
