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
/// The node reports its ledger crate's version requirement, such as `"=8.1.2"`,
/// so leading characters that are not digits are skipped.
///
/// The major number does not name the generation. Upstream renamed the ledger
/// crate for generation 9 and restarted its versions, so a ledger 9 node
/// reports `=1.0.0` while a ledger 8 node reports `=8.1.2`. The mapping below
/// is the observed one, not an arithmetic rule, and a later generation will
/// need its own entry rather than following a pattern.
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
        // `midnight-ledger` 8.x.
        8 => Ok(Generation::Ledger8),
        // `midnight-ledger-v9` 1.x: the crate line restarted at 1.0.0.
        1 => Ok(Generation::Ledger9),
        other => Err(GenerationError::Unsupported(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_strings_the_chains_actually_serve() {
        // Measured: preprod and mainnet served the first, preview the second,
        // and the ledger 9 devnet the third. The last is the trap: generation 9
        // reports major 1, because upstream restarted the crate's versions.
        assert_eq!(generation_of("=8.1.2"), Ok(Generation::Ledger8));
        assert_eq!(generation_of("=8.1.0"), Ok(Generation::Ledger8));
        assert_eq!(generation_of("=1.0.0"), Ok(Generation::Ledger9));
    }

    #[test]
    fn a_newer_ledger_names_itself_in_the_error() {
        // The upgrade prompt has to say which ledger, or an operator cannot
        // tell an old SDK from an unreachable node.
        let err = generation_of("=7.0.0").unwrap_err();
        assert_eq!(err, GenerationError::Unsupported(7));
        assert!(err.to_string().contains("ledger 7"));
    }

    #[test]
    fn a_version_with_no_number_is_reported_verbatim() {
        // An operator needs to see what the node actually said.
        let err = generation_of("unknown").unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }
}
