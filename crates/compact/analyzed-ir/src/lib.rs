//! Reader for `compiler/analyzed-ir.sexp`, the Compact compiler's analyzed
//! IR printed in its own vocabulary with each ledger operation expanded to
//! its Impact VM instructions.
//!
//! The grammar authority is the compiler itself: `compiler/langs.ss` for the
//! forms and `compiler/midnight-ledger.ss` for the instruction notation. This
//! crate parses that surface into a typed model and fails closed: an
//! unrecognized expression, type or operand form is an error naming the
//! form. The instruction set is the one deliberately open surface; parse it
//! generically and refuse unknown ops at execution time.
//!
//! The crate is `wasm32-unknown-unknown` compatible (no I/O in the API; the
//! caller supplies the artifact text), so an SDK in any language can embed it
//! behind a small bindings layer.

pub mod error;
pub mod model;
pub mod parse;
pub mod sexp;

pub use error::Error;
pub use model::*;

/// Parse one `analyzed-ir.sexp` artifact from its text.
pub fn parse_str(src: &str) -> Result<AnalyzedIr, Error> {
    let datum = sexp::read_one(src)?;
    parse::parse(&datum)
}
