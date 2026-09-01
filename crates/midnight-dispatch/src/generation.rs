//! Which ledger generation a chain runs.

use std::fmt;

/// A ledger generation this build can speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Generation {
    /// The generation preprod, preview and mainnet run.
    Ledger8,
    /// The generation the testnets take first.
    Ledger9,
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Generation::Ledger8 => f.write_str("ledger 8"),
            Generation::Ledger9 => f.write_str("ledger 9"),
        }
    }
}

/// Why a node's reported ledger version does not name a generation this build
/// can speak.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenerationError {
    /// The node reported a version with no leading major number.
    #[error("the node reported the ledger version {0:?}, which names no major version")]
    Unreadable(String),
    /// The node runs a generation this build does not carry.
    #[error(
        "the node runs ledger {0}, and this build carries ledger 8 and ledger 9. \
         Upgrade the SDK to reach this chain."
    )]
    Unsupported(u32),
}

/// The generation a node runs, from the string `midnight_ledgerVersion` returns.
///
/// The node reports a requirement rather than a bare version, such as `"=8.1.2"`
/// on preprod and mainnet, so leading characters that are not digits are
/// skipped. Only the major version decides the generation.
pub fn generation_of(reported: &str) -> Result<Generation, GenerationError> {
    let digits: String = reported
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let major: u32 = digits
        .parse()
        .map_err(|_| GenerationError::Unreadable(reported.to_owned()))?;
    match major {
        8 => Ok(Generation::Ledger8),
        9 => Ok(Generation::Ledger9),
        other => Err(GenerationError::Unsupported(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_requirement_form_the_node_actually_reports() {
        // The strings preprod, mainnet and preview served when measured.
        assert_eq!(generation_of("=8.1.2"), Ok(Generation::Ledger8));
        assert_eq!(generation_of("=8.1.0"), Ok(Generation::Ledger8));
        assert_eq!(generation_of("=9.0.0"), Ok(Generation::Ledger9));
    }

    #[test]
    fn a_newer_ledger_names_itself_in_the_error() {
        // The upgrade prompt has to say which ledger, or an operator cannot
        // tell an old SDK from an unreachable node.
        let err = generation_of("=10.0.0").unwrap_err();
        assert_eq!(err, GenerationError::Unsupported(10));
        assert!(err.to_string().contains("ledger 10"));
    }

    #[test]
    fn a_version_with_no_number_is_reported_verbatim() {
        // An operator needs to see what the node actually said.
        let err = generation_of("unknown").unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }
}
