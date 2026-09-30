//! Derive the interpreter's argument metadata from a circuit's signature.
//!
//! The funded call path executes a circuit's body against pre-encoded
//! `Value::AlignedValue` arguments. When the body destructures one of those
//! with a field access (e.g. `recipient.is_left`), the interpreter needs each
//! argument's declared type to slice it. A struct type carries its own field
//! list, so the layout follows from the type and needs no registry.

use crate::ir::{Argument, Type};

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
    use crate::ir::Ident;

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
}
