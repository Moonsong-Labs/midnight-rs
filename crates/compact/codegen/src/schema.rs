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
