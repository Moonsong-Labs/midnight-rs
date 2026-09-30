//! Contract transactions on a chain that runs ledger 9.
//!
//! The same source as [`crate::ledger_8`], compiled against ledger 9's types.
//! The public items are what a hand-built
//! [`ShieldedOffer::Ledger9`](crate::ShieldedOffer::Ledger9) is made of.

use midnight_helpers::ledger_9 as helpers;
use midnight_provider::ledger_9 as provider;
use midnight_types::ledger_9 as types;

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
pub use midnight_helpers::ledger_9::{
    BuildContext, InputInfo, OfferInfo, OutputInfo, ShieldedWallet,
};
pub use midnight_types::ledger_9::convert::IntoLedger;
pub use midnight_types::ledger_9::shielded_destination;

/// The generation this module builds for.
const LEDGER: midnight_types::LedgerVersion = midnight_types::LedgerVersion::V9;
