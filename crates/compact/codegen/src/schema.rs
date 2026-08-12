use std::path::Path;

use crate::types::ContractInfo;

pub fn parse_contract_info(path: &Path) -> Result<ContractInfo, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let info: ContractInfo = serde_json::from_str(&content)?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gateway_contract_info() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/fixtures/gateway-contract-info.json");
        let info = parse_contract_info(&path).expect("should parse");
        assert_eq!(info.circuits.len(), 6);
        assert_eq!(info.ledger.len(), 10);

        let threshold = info.ledger.iter().find(|f| f.name == "threshold").unwrap();
        assert_eq!(threshold.index_usize(), Some(0));
        assert_eq!(threshold.storage, crate::types::StorageKind::Cell);

        let egress = info
            .ledger
            .iter()
            .find(|f| f.name == "egress_jobs")
            .unwrap();
        assert_eq!(egress.index_usize(), Some(4));
        assert_eq!(egress.storage, crate::types::StorageKind::Map);
        assert!(egress.key.is_some());
        assert!(egress.value.is_some());

        let validators = info.ledger.iter().find(|f| f.name == "validators").unwrap();
        assert_eq!(validators.storage, crate::types::StorageKind::Set);

        let counter = info
            .ledger
            .iter()
            .find(|f| f.name == "next_job_id")
            .unwrap();
        assert_eq!(counter.storage, crate::types::StorageKind::Counter);
    }
}

#[cfg(test)]
mod analyzed_format_tests {
    use super::*;

    /// Every type node is tagged on `type-name` and no `structs` table is
    /// shipped, so a struct type must carry its own field list.
    #[test]
    fn parses_an_analyzed_artifact() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../tests/conformance/fixtures/bboard/compiler/contract-info.json"
        );
        let info = parse_contract_info(std::path::Path::new(path))
            .expect("analyzed contract-info should parse");

        assert!(info.circuits.iter().all(|c| c.ir.is_some()));

        let message = info
            .ledger
            .iter()
            .find(|f| f.name == "message")
            .expect("message field");
        let Some(crate::types::TypeNode::Struct { elements, .. }) = &message.element_type else {
            panic!("message should be a struct-typed cell")
        };
        assert_eq!(elements.len(), 2, "Maybe carries its fields inline");
    }
}

#[cfg(test)]
mod wide_uint_tests {
    /// Adding two `Uint<64>` values produces an intermediate bound above
    /// `u64`. Both load paths must carry it: the whole document, and a single
    /// circuit body pulled out of an already-parsed value.
    #[test]
    fn loads_a_body_whose_bound_exceeds_u64() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/wide-uint-contract-info.json"
        ));
        let raw: serde_json::Value = serde_json::from_str(text).expect("json");
        let info: crate::types::ContractInfo = serde_json::from_str(text).expect("whole document");
        assert_eq!(info.circuits.len(), 1);

        let ir = &raw["circuits"][0]["ir"];
        let body: crate::ir::CircuitIrBody =
            crate::ir::from_json_value(ir).expect("body from an already-parsed value");
        assert!(matches!(body.body, crate::ir::Stmt::Seq { .. }));
    }
}
