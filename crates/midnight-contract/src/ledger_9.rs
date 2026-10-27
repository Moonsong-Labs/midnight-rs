//! Contract transactions on a chain that runs ledger 9.
//!
//! The same source as `ledger_8`, compiled against ledger 9's types.

use midnight_helpers::ledger_9 as helpers;
use midnight_provider::ledger_9 as provider;
use midnight_types::ledger_9 as types;
use types::LEDGER;

#[path = "per_ledger/call.rs"]
pub(crate) mod call;
#[path = "per_ledger/deploy.rs"]
pub(crate) mod deploy;
#[path = "per_ledger/maintenance.rs"]
pub(crate) mod maintenance;
#[path = "per_ledger/resolver.rs"]
mod resolver;
#[path = "per_ledger/state.rs"]
pub(crate) mod state;
