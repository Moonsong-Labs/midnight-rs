//! Contract transactions on a chain that runs ledger 8.
//!
//! The same source as [`crate::ledger_9`], compiled against ledger 8's types.
//! The public items are what a hand-built
//! [`ShieldedOffer::Ledger8`](crate::ShieldedOffer::Ledger8) is made of.

use midnight_helpers::ledger_8 as helpers;
use midnight_provider::ledger_8 as provider;
use midnight_types::ledger_8 as types;

#[path = "per_ledger/call.rs"]
pub(crate) mod call;
#[path = "per_ledger/deploy.rs"]
pub(crate) mod deploy;
#[path = "per_ledger/maintenance.rs"]
pub(crate) mod maintenance;
#[path = "per_ledger/resolver.rs"]
mod resolver;
#[path = "per_ledger/state.rs"]
mod state;

pub use midnight_helpers::DefaultDB;
pub use midnight_helpers::ledger_8::{
    BuildContext, InputInfo, OfferInfo, OutputInfo, ShieldedWallet,
};
pub use midnight_types::ledger_8::convert::IntoLedger;
pub use midnight_types::ledger_8::shielded_destination;

/// The generation this module builds for.
const LEDGER: midnight_types::LedgerVersion = midnight_types::LedgerVersion::V8;
