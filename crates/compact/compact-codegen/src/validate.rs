//! Pre-expansion validation of a parsed `contract-info.json`.
//!
//! Runs before any code is generated so every problem surfaces as a single
//! precise compile error instead of a panic, a silent fallback, or broken
//! generated code:
//!
//! - the schema version gate (see [`crate::types::check_versions`]),
//! - rejection of unrecognized `type-name`s ([`TypeNode::Unknown`]),
//! - a round-trip check of the IR / helper / struct / enum definitions that
//!   are embedded as JSON string constants in the generated code.

use crate::error::CodegenError;
use crate::ir::{CircuitIrBody, EnumDef, HelperDef, StructDef};
use crate::types::{ContractInfo, TypeNode};

/// Validate a parsed `contract-info.json` before expansion.
pub fn validate(info: &ContractInfo) -> Result<(), CodegenError> {
    crate::types::check_versions(info)?;
    check_unknown_types(info)?;
    check_embedded_json(info)?;
    Ok(())
}

/// Fail on any [`TypeNode::Unknown`] reachable from ledger fields, circuit
/// signatures, or witness signatures, naming the unrecognized `type-name`
/// and the path to the field that used it.
fn check_unknown_types(info: &ContractInfo) -> Result<(), CodegenError> {
    for field in &info.ledger {
        let base = format!("ledger field `{}`", field.name);
        if let Some(t) = &field.element_type {
            check_type(t, &base)?;
        }
        if let Some(t) = &field.key {
            check_type(t, &format!("{base} key type"))?;
        }
        if let Some(t) = &field.value {
            check_type(t, &format!("{base} value type"))?;
        }
    }
    for circuit in &info.circuits {
        for arg in &circuit.arguments {
            check_type(
                &arg.type_node,
                &format!("circuit `{}` argument `{}`", circuit.name, arg.name),
            )?;
        }
        check_type(
            &circuit.result_type,
            &format!("circuit `{}` result type", circuit.name),
        )?;
    }
    for witness in &info.witnesses {
        for arg in &witness.arguments {
            check_type(
                &arg.type_node,
                &format!("witness `{}` argument `{}`", witness.name, arg.name),
            )?;
        }
        check_type(
            &witness.result_type,
            &format!("witness `{}` result type", witness.name),
        )?;
    }
    Ok(())
}

fn check_type(node: &TypeNode, location: &str) -> Result<(), CodegenError> {
    match node {
        TypeNode::Unknown { type_name } => Err(CodegenError::UnknownTypeName {
            type_name: type_name.clone(),
            location: location.to_string(),
        }),
        TypeNode::Vector { inner, .. } => check_type(inner, &format!("{location}, vector element")),
        TypeNode::Tuple { types } => {
            for (i, t) in types.iter().enumerate() {
                check_type(t, &format!("{location}, tuple element {i}"))?;
            }
            Ok(())
        }
        TypeNode::Struct { name, elements } => {
            for element in elements {
                check_type(
                    &element.type_node,
                    &format!("{location}, struct `{name}` field `{}`", element.name),
                )?;
            }
            Ok(())
        }
        TypeNode::Alias { name, inner } => {
            check_type(inner, &format!("{location}, alias `{name}`"))
        }
        TypeNode::Boolean
        | TypeNode::Field
        | TypeNode::Uint { .. }
        | TypeNode::Bytes { .. }
        | TypeNode::Enum { .. }
        | TypeNode::Opaque { .. }
        | TypeNode::Contract { .. } => Ok(()),
    }
}

/// Round-trip the definitions that `expand::circuit_calls` embeds as JSON
/// string constants. After this check the emitters can serialize them
/// infallibly, and the slim runtime re-parse in the generated circuit methods
/// only fails if the embedded constant was tampered with.
fn check_embedded_json(info: &ContractInfo) -> Result<(), CodegenError> {
    for circuit in &info.circuits {
        if circuit.pure {
            continue;
        }
        if let Some(ir) = &circuit.ir {
            round_trip::<CircuitIrBody>(ir, format!("IR for circuit `{}`", circuit.name))?;
        }
    }
    round_trip::<Vec<HelperDef>>(&info.helpers, "helper definitions".to_string())?;
    round_trip::<Vec<StructDef>>(&info.structs, "struct definitions".to_string())?;
    let enums = crate::expand::circuit_calls::collect_enum_defs(info);
    round_trip::<Vec<EnumDef>>(&enums, "enum definitions".to_string())?;
    Ok(())
}

fn round_trip<T: serde::de::DeserializeOwned>(
    value: &impl serde::Serialize,
    what: String,
) -> Result<(), CodegenError> {
    let json = serde_json::to_string(value).map_err(|source| CodegenError::EmbedJson {
        what: what.clone(),
        source,
    })?;
    serde_json::from_str::<T>(&json).map_err(|source| CodegenError::EmbedJson { what, source })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_from_json(json: &str) -> ContractInfo {
        serde_json::from_str(json).expect("test JSON should parse")
    }

    fn minimal_info(compiler: &str, language: &str, ledger_type: &str) -> ContractInfo {
        info_from_json(&format!(
            r#"{{
                "compiler-version": "{compiler}",
                "language-version": "{language}",
                "runtime-version": "0.16.101",
                "circuits": [],
                "witnesses": [],
                "contracts": [],
                "ledger": [
                    {{
                        "name": "count",
                        "index": 0,
                        "storage": "Cell",
                        "exported": true,
                        "type": {ledger_type}
                    }}
                ]
            }}"#
        ))
    }

    #[test]
    fn accepts_supported_version_families() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        validate(&minimal_info("0.30.102", "0.22.101", bool_cell)).expect("0.30/0.22 supported");
        validate(&minimal_info("0.31.104", "0.23.104", bool_cell)).expect("0.31/0.23 supported");
    }

    #[test]
    fn rejects_out_of_range_compiler_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("0.29.107", "0.22.101", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("compiler-version"), "names the field: {msg}");
        assert!(msg.contains("0.29.107"), "names the found value: {msg}");
        assert!(
            msg.contains("0.30.x, 0.31.x"),
            "names the supported range: {msg}"
        );

        let err = validate(&minimal_info("9.99.0", "0.22.101", bool_cell)).unwrap_err();
        assert!(err.to_string().contains("9.99.0"));
    }

    #[test]
    fn rejects_out_of_range_language_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("0.31.104", "0.99.0", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("language-version"), "names the field: {msg}");
        assert!(msg.contains("0.99.0"), "names the found value: {msg}");
        assert!(
            msg.contains("0.22.x, 0.23.x"),
            "names the supported range: {msg}"
        );
    }

    #[test]
    fn rejects_malformed_version() {
        let bool_cell = r#"{ "type-name": "Boolean" }"#;
        let err = validate(&minimal_info("nightly", "0.22.101", bool_cell)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed compiler-version"), "{msg}");
        assert!(msg.contains("nightly"), "{msg}");
    }

    #[test]
    fn rejects_unknown_type_in_ledger_field() {
        let err = validate(&minimal_info(
            "0.31.104",
            "0.23.104",
            r#"{ "type-name": "FancyFutureType" }"#,
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`FancyFutureType`"), "names the type: {msg}");
        assert!(
            msg.contains("ledger field `count`"),
            "names the path: {msg}"
        );
    }

    #[test]
    fn rejects_unknown_type_nested_in_circuit_argument() {
        let info = info_from_json(
            r#"{
                "compiler-version": "0.31.104",
                "language-version": "0.23.104",
                "runtime-version": "0.16.101",
                "circuits": [
                    {
                        "name": "vote",
                        "pure": false,
                        "proof": true,
                        "arguments": [
                            {
                                "name": "ballot",
                                "type": {
                                    "type-name": "Struct",
                                    "name": "Ballot",
                                    "elements": [
                                        {
                                            "name": "choice",
                                            "type": { "type-name": "Quantum" }
                                        }
                                    ]
                                }
                            }
                        ],
                        "result-type": { "type-name": "Tuple", "types": [] }
                    }
                ],
                "witnesses": [],
                "contracts": [],
                "ledger": []
            }"#,
        );
        let err = validate(&info).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`Quantum`"), "names the type: {msg}");
        assert!(
            msg.contains("circuit `vote` argument `ballot`"),
            "names the circuit and argument: {msg}"
        );
        assert!(
            msg.contains("struct `Ballot` field `choice`"),
            "names the struct field: {msg}"
        );
    }

    #[test]
    fn version_gate_runs_before_unknown_type_check() {
        // Both problems present: the version error wins, so trybuild
        // fixtures for each failure mode stay independent.
        let err = validate(&minimal_info(
            "0.29.107",
            "0.22.101",
            r#"{ "type-name": "FancyFutureType" }"#,
        ))
        .unwrap_err();
        assert!(err.to_string().contains("compiler-version"));
    }
}
