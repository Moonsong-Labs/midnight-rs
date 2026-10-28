use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::ir::Type;
use crate::types::{Circuit, FieldIndex, LedgerField, StorageKind};

use super::circuit_calls::typed_params;
use super::circuits::return_type_ident;
use super::constants::field_const_ident;
use super::emit_ir::type_ref;
use super::helpers::{make_ident, to_pascal_case};
use super::types::{encode_to_aligned_value, type_to_tokens};

/// The name of the lazy query struct of the contract `contract_name`.
fn query_struct_ident(contract_name: &str) -> Ident {
    format_ident!("{}Query", contract_name)
}

pub(crate) fn emit_ledger_wrapper(
    fields: &[LedgerField],
    name: &str,
    ir_constants: &TokenStream,
    info: &crate::types::ContractInfo,
) -> TokenStream {
    let struct_name = format_ident!("{}", name);
    let query_struct_name = query_struct_ident(name);

    // Circuits that carry an on-chain entry point, and so a verifier key.
    // Pure circuits are evaluated off-chain and have neither.
    let declared_circuits: Vec<&str> = info
        .circuits
        .iter()
        .filter(|c| !c.pure())
        .map(|c| c.name.as_str())
        .collect();

    // Witnesses read and write private state, so calls on this contract need a
    // private-state provider attached to be meaningful.
    let declares_witnesses = !info.witnesses.is_empty();

    let accessors: Vec<_> = fields
        .iter()
        .map(|field| {
            let const_name = field_const_ident(&field.name);
            emit_field_accessor(field, &const_name)
        })
        .collect();

    // Pure functions are inlined by the compiler, so no helper registry is
    // needed.

    // Access to the underlying state for advanced use.
    // Named contract_state to avoid conflicts with ledger fields named "state".
    let state_accessor = quote! {
        /// Access the underlying `ContractState`.
        pub fn contract_state(&self) -> &ContractState<InMemoryDB> {
            &self.state
        }

        /// Consume this wrapper and return the underlying `ContractState`.
        pub fn into_contract_state(self) -> ContractState<InMemoryDB> {
            self.state
        }
    };

    // Generate InitialState struct with typed fields
    let initial_state = emit_initial_state(fields, name, info.has_constructor);

    // Generate Circuits struct with async on-chain call methods
    let circuit_methods_struct = emit_circuits_struct(info, &struct_name);

    quote! {
        /// Typed access to the contract's ledger state and circuit calls.
        pub struct #struct_name {
            state: ContractState<InMemoryDB>,
        }

        impl #struct_name {
            /// Create from a deserialized `ContractState`.
            pub fn new(state: ContractState<InMemoryDB>) -> Self {
                Self { state }
            }

            /// Create from a hex-encoded contract state string (as the node RPC returns it).
            pub fn from_hex(hex_state: &str) -> Result<Self, StateError> {
                let bytes = hex::decode(hex_state).map_err(|e| StateError::HexDecode(e.to_string()))?;
                Ok(Self::new(decode_contract_state(&bytes)?))
            }

            /// Fetch the contract state at the node's best block from a provider and wrap it.
            pub async fn from_provider<P: midnight_contract::Provider>(
                provider: &P,
                address: &str,
            ) -> Result<Self, midnight_contract::ContractError> {
                let state = midnight_contract::state::fetch_state(provider, address, None).await?;
                Ok(Self::new(state))
            }

            #state_accessor

            #(#accessors)*

            #ir_constants
        }

        #initial_state

        /// A deployed contract instance with typed circuit call methods.
        ///
        /// # Example
        ///
        /// ```rust,ignore
        /// let contract = Contract::deploy(&provider)
        ///     .with_initial_state(LedgerInitialState::default())
        ///     .with_zk_config("compiled")
        ///     .await?;
        ///
        /// contract.circuits().increment().await?;
        /// let ledger = contract.ledger().await?;
        /// ```
        pub struct Contract<P>(midnight_contract::Contract<P>);

        impl Contract<()> {
            /// Start building a deployment for this contract.
            ///
            /// Returns a `DeployBuilder` that can be awaited directly. The
            /// provider must have a synced wallet attached via
            /// `MidnightProvider::with_wallet(...)`.
            pub fn deploy<'a, P>(provider: P) -> DeployBuilder<'a, P>
            where
                P: midnight_contract::AsMidnightProvider,
            {
                // Typed so a contract with no impure circuits still infers.
                const DECLARED_CIRCUITS: &[&str] = &[#(#declared_circuits),*];
                DeployBuilder(
                    midnight_contract::Contract::deploy(provider)
                        .with_declared_circuits(DECLARED_CIRCUITS.iter().copied())
                        .with_declared_witnesses(#declares_witnesses),
                )
            }

            /// Create a handle for an already-deployed contract at the given
            /// contract address: a hex string or a typed `ContractAddress`.
            ///
            /// This is synchronous, no network calls are made. Call `.build()`
            /// on the returned builder to get the `Contract<P>` handle.
            pub fn at<P>(
                provider: P,
                address: impl midnight_contract::IntoContractAddress,
            ) -> ConnectBuilder<P>
            where
                P: midnight_contract::AsMidnightProvider,
            {
                ConnectBuilder(
                    midnight_contract::Contract::at(provider, address)
                        .with_declared_witnesses(#declares_witnesses),
                )
            }
        }

        /// Builder wrapper around `midnight_contract::DeployBuilder` that
        /// yields the generated `Contract<P>` on deploy.
        #[must_use = "does nothing until awaited or sent"]
        pub struct DeployBuilder<'a, P>(midnight_contract::DeployBuilder<'a, P>);

        impl<P> DeployBuilder<'_, P> {
            /// Set the initial contract state.
            pub fn with_initial_state(self, state: impl Into<ContractState<InMemoryDB>>) -> Self {
                Self(self.0.with_initial_state(state))
            }

            /// Attach a shielded offer that the deploy transaction carries: the
            /// deploy spends the offer's coins from the attached wallet and pays
            /// each of its outputs. See
            /// [`midnight_contract::DeployBuilder::with_shielded_offer`].
            pub fn with_shielded_offer(self, offer: midnight_contract::ShieldedOffer) -> Self {
                Self(self.0.with_shielded_offer(offer))
            }

            /// Make the deployed contract governable by setting its maintenance
            /// authority committee (verifying keys) and threshold. See
            /// [`midnight_contract::DeployBuilder::with_maintenance_authority`].
            pub fn with_maintenance_authority(
                self,
                committee: Vec<midnight_contract::VerifyingKey>,
                threshold: u32,
            ) -> Self {
                Self(self.0.with_maintenance_authority(committee, threshold))
            }

            /// Set the source of the contract's compiled ZK artifacts (a directory
            /// path or a custom `ZkConfigProvider`); see `midnight_contract::IntoZkConfig`.
            pub fn with_zk_config(self, zk_config: impl midnight_contract::IntoZkConfig) -> Self {
                Self(self.0.with_zk_config(zk_config))
            }

            /// Set the deadline of `midnight_contract::PendingDeploy::into_contract`
            /// (default: 60s).
            ///
            /// The deadline starts when `into_contract` starts, after `send` returns.
            /// It bounds the best-block wait that `into_contract` does when no wait
            /// ran before it, and then the read of the deployed contract. The SDK sets
            /// no deadline on an explicit `wait_best` or `wait_finalized`. See
            /// `midnight_contract::PendingTx` to bound one with `tokio::time::timeout`.
            pub fn with_deploy_timeout(self, timeout: std::time::Duration) -> Self {
                Self(self.0.with_deploy_timeout(timeout))
            }

            /// Submit the deploy transaction and return a `PendingDeploy` handle.
            ///
            /// Use [`PendingDeploy::wait_best`] / [`PendingDeploy::wait_finalized`]
            /// to observe inclusion states, then [`PendingDeploy::into_contract`]
            /// to obtain the typed `Contract<P>`. Each wait fails with
            /// `ContractError::TransactionFailed` when the chain did not apply
            /// the deploy. `into_contract` waits for the best block itself when
            /// no wait ran, and then reads the deployed contract from the node.
            pub async fn send(self) -> Result<PendingDeploy<P>, midnight_contract::ContractError>
            where
                P: midnight_contract::AsMidnightProvider + Send,
            {
                // Boxed for the same reason the `IntoFuture` impl below is.
                Box::pin(async move { Ok(PendingDeploy(self.0.send().await?)) }).await
            }
        }

        impl<'a, P> std::future::IntoFuture for DeployBuilder<'a, P>
        where
            P: midnight_contract::AsMidnightProvider + Send + 'a,
        {
            type Output = Result<Contract<P>, midnight_contract::ContractError>;
            type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>>;

            fn into_future(self) -> Self::IntoFuture {
                Box::pin(async move { self.0.await.map(Contract) })
            }
        }

        /// Wrapper around `midnight_contract::PendingDeploy` that yields the
        /// generated `Contract<P>` from `into_contract`.
        pub struct PendingDeploy<P>(midnight_contract::PendingDeploy<P>);

        impl<P> PendingDeploy<P> {
            /// The contract address the deploy will produce.
            pub fn address(&self) -> &str {
                self.0.address()
            }

            /// The hash of the submitted extrinsic.
            pub fn extrinsic_hash(&self) -> midnight_contract::ExtrinsicHash {
                self.0.extrinsic_hash()
            }

            /// The extrinsic hash formatted as a hex string (no `0x` prefix).
            pub fn extrinsic_hash_hex(&self) -> String {
                self.0.extrinsic_hash_hex()
            }

            /// The hash of the Midnight transaction the deploy extrinsic
            /// carries, the identity the indexer keys on.
            pub fn transaction_hash(&self) -> midnight_contract::TransactionHash {
                self.0.transaction_hash()
            }

            /// Wait until the deploy transaction lands in the best block, and
            /// return the inclusion when the chain applied it there.
            ///
            /// Consumes `self` and returns it back so callers can chain. Fails
            /// with `ContractError::TransactionFailed` when the chain did not
            /// apply the deploy.
            pub async fn wait_best(
                self,
            ) -> Result<(midnight_contract::TxInBlock, Self), midnight_contract::ContractError> {
                let (in_block, inner) = self.0.wait_best().await?;
                Ok((in_block, Self(inner)))
            }

            /// Wait until the deploy transaction is in a finalized block, and
            /// return the inclusion when the chain applied it there.
            ///
            /// Consumes `self` and returns it back. May be called without a
            /// prior `wait_best`; the best-block status is then skipped. Fails
            /// with `ContractError::TransactionFailed` when the chain did not
            /// apply the deploy.
            pub async fn wait_finalized(
                self,
            ) -> Result<(midnight_contract::TxInBlock, Self), midnight_contract::ContractError> {
                let (in_block, inner) = self.0.wait_finalized().await?;
                Ok((in_block, Self(inner)))
            }
        }

        impl<P> PendingDeploy<P>
        where
            P: midnight_contract::AsMidnightProvider + Send,
        {
            /// Read the deployed contract from the node, at the block that applied the
            /// deploy, and return the typed `Contract<P>`.
            ///
            /// When no `wait_best` or `wait_finalized` ran, `into_contract`
            /// first waits for the best block itself, under the same deadline
            /// as the read. See `midnight_contract::PendingDeploy::into_contract`.
            pub async fn into_contract(self) -> Result<Contract<P>, midnight_contract::ContractError> {
                self.0.into_contract().await.map(Contract)
            }
        }

        /// Builder wrapper around `midnight_contract::ConnectBuilder` that
        /// yields the generated `Contract<P>` on build.
        #[must_use = "call .build() to get the contract handle"]
        pub struct ConnectBuilder<P>(midnight_contract::ConnectBuilder<P>);

        impl<P> ConnectBuilder<P> {
            /// Set the source of the contract's compiled ZK artifacts (a directory
            /// path or a custom `ZkConfigProvider`); see `midnight_contract::IntoZkConfig`.
            pub fn with_zk_config(self, zk_config: impl midnight_contract::IntoZkConfig) -> Self {
                Self(self.0.with_zk_config(zk_config))
            }

            /// Pin queries to the block `hash`. Default is the node's best block.
            /// Both circuit calls and lazy ledger queries honour the pin through the
            /// node RPC, and a circuit call runs at the time of that block.
            pub fn at_block(self, hash: midnight_contract::BlockHash) -> Self {
                Self(self.0.at_block(hash))
            }

            /// Build the contract handle. This is synchronous.
            pub fn build(self) -> Contract<P>
            where
                P: midnight_contract::AsMidnightProvider,
            {
                Contract(self.0.build())
            }
        }

        impl<P> Contract<P> {
            /// The contract's on-chain address (hex string).
            pub fn address(&self) -> &str {
                self.0.address()
            }

            /// Access on-chain circuit call methods.
            ///
            /// Defaults to no witnesses; chain [`Circuits::with_witnesses`] for
            /// circuits that call witnesses.
            pub fn circuits(&self) -> Circuits<'_, P> {
                Circuits {
                    contract: &self.0,
                    witnesses: midnight_contract::runtime::NoWitnesses,
                    coin_encryption_keys: Vec::new(),
                    shielded: ::core::default::Default::default(),
                }
            }
        }

        impl<P> Contract<P>
        where
            P: midnight_contract::AsMidnightProvider,
        {
            /// Fetch the current ledger state from the node.
            ///
            /// Returns the sync `Ledger` struct with typed field accessors.
            /// Uses the `midnight_contractState` node RPC which is available
            /// on all standard devnet nodes.
            pub async fn ledger(&self) -> Result<#struct_name, midnight_contract::ContractError> {
                let provider = self.0.provider().as_midnight_provider();
                let state = midnight_contract::state::fetch_state(
                    provider,
                    self.0.address(),
                    self.0.at_block(),
                ).await?;
                Ok(#struct_name::new(state))
            }

            /// Maintenance / governance operations (verifier-key rotation,
            /// authority replacement). See
            /// [`midnight_contract::Contract::maintenance`].
            pub fn maintenance(&self) -> midnight_contract::ContractMaintenance<'_, P> {
                self.0.maintenance()
            }

            /// Read the current maintenance authority (committee, threshold,
            /// counter) from on-chain state.
            pub async fn maintenance_authority(
                &self,
            ) -> Result<
                midnight_contract::ContractMaintenanceAuthority,
                midnight_contract::ContractError,
            > {
                self.0.maintenance_authority().await
            }
        }

        impl<P> Contract<P>
        where
            for<'p> &'p P: lazy::StateQueryProvider,
        {
            /// Get a lazy query handle for per-field state access.
            ///
            /// This uses the `midnight_queryContractState` node RPC which is
            /// only available on custom node builds. For standard devnet nodes,
            /// use `ledger()` instead.
            pub fn ledger_query(&self) -> #query_struct_name<&P> {
                #query_struct_name::new(self.0.provider(), self.0.address().to_string(), self.0.at_block())
            }
        }

        impl<P> From<midnight_contract::Contract<P>> for Contract<P> {
            fn from(inner: midnight_contract::Contract<P>) -> Self {
                Contract(inner)
            }
        }

        #circuit_methods_struct
    }
}

fn emit_field_accessor(field: &LedgerField, const_name: &Ident) -> TokenStream {
    let method_name = make_ident(&field.name);
    let field_indices = field_indices_expr(const_name, &field.index);
    let nav = quote! { get_field_path(self.state.data.get_ref(), #field_indices) };
    let doc = format!(
        "Access the `{}` ledger field ({}).",
        field.name, field.storage
    );

    match field.storage {
        StorageKind::Cell => {
            emit_cell_accessor(&method_name, &doc, &nav, field.element_type.as_ref())
        }
        StorageKind::Counter => emit_typed_cell_accessor(&method_name, &doc, &nav, &quote! { u64 }),
        StorageKind::Map => {
            let key_ty = storage_arg_tokens(field.key.as_ref());
            let val_ty = storage_arg_tokens(field.value.as_ref());
            let accessor = quote! { MapAccessor<'_, #key_ty, #val_ty> };
            emit_from_state_accessor(&method_name, &doc, &nav, &accessor)
        }
        StorageKind::Set => {
            let elem_ty = storage_arg_tokens(field.element_type.as_ref());
            let accessor = quote! { SetAccessor<'_, #elem_ty> };
            emit_from_state_accessor(&method_name, &doc, &nav, &accessor)
        }
        StorageKind::List => {
            let elem_ty = storage_arg_tokens(field.element_type.as_ref());
            let accessor = quote! { ListAccessor<'_, #elem_ty> };
            emit_from_state_accessor(&method_name, &doc, &nav, &accessor)
        }
        StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => {
            emit_from_state_accessor(&method_name, &doc, &nav, &quote! { MerkleTreeAccessor<'_> })
        }
    }
}

fn emit_cell_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    cell_type: Option<&Type>,
) -> TokenStream {
    match cell_type {
        Some(ty) => emit_typed_cell_accessor(method_name, doc, nav, &type_to_tokens(ty)),
        None => quote! {
            #[doc = #doc]
            pub fn #method_name(&self) -> Result<&StateValue<InMemoryDB>, StateError> {
                #nav
            }
        },
    }
}

fn emit_typed_cell_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    ret_type: &TokenStream,
) -> TokenStream {
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<#ret_type, StateError> {
            cell_as::<#ret_type>(#nav?)
        }
    }
}

fn emit_from_state_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    accessor: &TokenStream,
) -> TokenStream {
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<#accessor, StateError> {
            <#accessor>::from_state(#nav?)
        }
    }
}

/// The Rust type of a storage type argument, or `Vec<u8>` for an absent one.
fn storage_arg_tokens(ty: Option<&Type>) -> TokenStream {
    ty.map_or_else(|| quote! { Vec<u8> }, type_to_tokens)
}

// ---------------------------------------------------------------------------
// InitialState: typed struct for contract deployment
// ---------------------------------------------------------------------------

/// The deploy state for ledger slots addressed by their artifact index paths.
///
/// Once a ledger passes fifteen fields compactc stops putting them in one
/// `StateValue::Array` — the runtime caps an array at sixteen children — and
/// nests them instead, addressing every cell by an index *path*, `(0 0)` or
/// `(1 12)`, which is also how the compiled circuits read it. Grouping the
/// slots by their leading index and recursing rebuilds that same shape. A
/// contract whose cells all sit at one level keeps the flat array it built
/// before — now ordered by index rather than by declaration, which is the
/// same order for every artifact compactc emits.
fn state_array_expr(slots: Vec<(Vec<usize>, TokenStream)>) -> TokenStream {
    // The recursion has consumed this slot's whole path: it is the value.
    if slots.len() == 1 && slots[0].0.is_empty() {
        let value = &slots[0].1;
        return quote! { #value };
    }

    let mut groups: std::collections::BTreeMap<usize, Vec<(Vec<usize>, TokenStream)>> =
        std::collections::BTreeMap::new();
    for (path, conversion) in slots {
        let Some((&first, rest)) = path.split_first() else {
            let msg = "a ledger field's index path stops short of its siblings'";
            return quote! { compile_error!(#msg) };
        };
        groups
            .entry(first)
            .or_default()
            .push((rest.to_vec(), conversion));
    }

    // By index, not by declaration: slot `i` holds the field the artifact
    // declares at `i`.
    let elements: Vec<TokenStream> = (0..groups.len())
        .map(|i| match groups.remove(&i) {
            Some(group) => state_array_expr(group),
            None => {
                let msg = format!("the artifact declares no ledger field at state index {i}");
                quote! { compile_error!(#msg) }
            }
        })
        .collect();

    quote! { StateValue::Array(vec![#(#elements),*].into()) }
}

/// The doc of `{Name}InitialState`, with the constructor note when the
/// contract has a constructor.
fn initial_state_doc(has_constructor: bool) -> TokenStream {
    let mut lines = vec![
        " Initial state for deploying this contract.",
        "",
        " `Default` gives each field the compiler's initial value for its type.",
    ];
    if has_constructor {
        lines.extend([
            "",
            " The SDK does not run the contract's constructor, so this state is the",
            " state before the constructor runs. Before you deploy, set each field that",
            " the constructor writes to the value that the constructor writes.",
        ]);
    }
    quote! { #(#[doc = #lines])* }
}

fn emit_initial_state(fields: &[LedgerField], name: &str, has_constructor: bool) -> TokenStream {
    let struct_name = format_ident!("{}InitialState", name);
    let ledger_name = format_ident!("{}", name);
    let doc = initial_state_doc(has_constructor);

    let mut field_defs = Vec::new();
    let mut field_defaults = Vec::new();
    let mut field_conversions: Vec<(Vec<usize>, TokenStream)> = Vec::new();

    for field in fields {
        let field_name = make_ident(&field.name);
        let doc = format!("Initial value for `{}`.", field.name);
        // Where the artifact puts this field: one index when the ledger
        // fits a single array, a path into the nested state otherwise.
        let field_path: Vec<usize> = match &field.index {
            FieldIndex::Single(index) => vec![*index],
            FieldIndex::Path(path) => path.clone(),
        };

        let conversion = match field.storage {
            StorageKind::Cell => match &field.element_type {
                // Typed fields only for simple scalar types that have
                // Default + Into<AlignedValue>. Other types use AlignedValue.
                Some(ty @ (Type::Unsigned(_) | Type::Boolean)) => {
                    let rust_type = type_to_tokens(ty);
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: #rust_type });
                    field_defaults.push(quote! { #field_name: Default::default() });
                    quote! { StateValue::from(AlignedValue::from(self.#field_name)) }
                }
                Some(ty) => {
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: AlignedValue });
                    let ty = type_ref(ty);
                    field_defaults.push(quote! {
                        #field_name: midnight_contract::runtime::default_aligned(&#ty)
                    });
                    quote! { StateValue::from(self.#field_name.clone()) }
                }
                None => {
                    let msg = format!("ledger cell `{}` carries no type", field.name);
                    quote! { compile_error!(#msg) }
                }
            },
            StorageKind::Counter => {
                field_defs.push(quote! { #[doc = #doc] pub #field_name: u64 });
                field_defaults.push(quote! { #field_name: 0 });
                quote! { StateValue::from(self.#field_name) }
            }
            StorageKind::Map | StorageKind::Set => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StorageHashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StorageHashMap::new() });
                quote! { StateValue::Map(self.#field_name) }
            }
            StorageKind::List => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                field_defaults.push(quote! {
                    #field_name: StateValue::Array(
                        vec![StateValue::Null, StateValue::Null, StateValue::from(0u64)].into(),
                    )
                });
                quote! { self.#field_name }
            }
            StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                let default = merkle_tree_default(field);
                field_defaults.push(quote! { #field_name: #default });
                quote! { self.#field_name }
            }
        };

        field_conversions.push((field_path, conversion));
    }

    let state_expr = state_array_expr(field_conversions);
    let definition = if fields.is_empty() {
        quote! {
            #[derive(Debug, Clone, Default)]
            pub struct #struct_name;
        }
    } else {
        let model_imports = super::circuit_calls::model_imports();
        quote! {
            #[derive(Debug, Clone)]
            pub struct #struct_name {
                #(#field_defs),*
            }

            impl Default for #struct_name {
                fn default() -> Self {
                    #model_imports
                    Self {
                        #(#field_defaults),*
                    }
                }
            }
        }
    };

    quote! {
        #doc
        #definition

        impl #struct_name {
            /// Build the `ContractState` for deployment.
            pub fn build(self) -> ContractState<InMemoryDB> {
                ContractState::new(
                    #state_expr,
                    StorageHashMap::new(),
                    ContractMaintenanceAuthority::default(),
                )
            }

            /// Build and wrap in the typed Ledger.
            pub fn into_ledger(self) -> #ledger_name {
                #ledger_name::new(self.build())
            }
        }

        impl From<#struct_name> for ContractState<InMemoryDB> {
            fn from(state: #struct_name) -> Self {
                state.build()
            }
        }
    }
}

/// The initial state of a `MerkleTree` or `HistoricMerkleTree` field.
///
/// The compiler's generated `initialState` runs the `resetToDefault` of each
/// field's ADT (`compiler/midnight-ledger.ss`). For a tree, that writes a blank
/// tree of the field's depth and a first free index of 0. A
/// `HistoricMerkleTree` also records the blank tree's root in its map of past
/// roots.
fn merkle_tree_default(field: &LedgerField) -> TokenStream {
    let Some(depth) = field.depth.and_then(|depth| u8::try_from(depth).ok()) else {
        let msg = format!("Merkle tree `{}` has no depth that fits a u8", field.name);
        return quote! { compile_error!(#msg) };
    };
    if field.storage == StorageKind::MerkleTree {
        return quote! {
            StateValue::Array(
                vec![
                    StateValue::BoundedMerkleTree(MerkleTree::blank(#depth)),
                    StateValue::from(0u64),
                ]
                .into(),
            )
        };
    }
    // A blank tree's root is always `Some`. The `if let` keeps a panic out of
    // the generated code.
    quote! {
        {
            let tree: MerkleTree<(), InMemoryDB> = MerkleTree::blank(#depth);
            let mut past_roots: StorageHashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB> =
                StorageHashMap::new();
            if let Some(root) = tree.root() {
                past_roots = past_roots.insert(AlignedValue::from(root), StateValue::Null);
            }
            StateValue::Array(
                vec![
                    StateValue::BoundedMerkleTree(tree),
                    StateValue::from(0u64),
                    StateValue::Map(past_roots),
                ]
                .into(),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Lazy wrapper (query-per-field via Provider)
// ---------------------------------------------------------------------------

pub(crate) fn emit_lazy_ledger_wrapper(fields: &[LedgerField], name: &str) -> TokenStream {
    let struct_name = query_struct_ident(name);

    let accessors: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let const_name = field_const_ident(&field.name);
            emit_lazy_field_accessor(field, &const_name)
        })
        .collect();

    quote! {
        /// Lazy query interface — each accessor calls the RPC to fetch only
        /// the requested field instead of downloading the full contract state.
        pub struct #struct_name<P: lazy::StateQueryProvider> {
            address: String,
            provider: P,
            at_block_hash: Option<midnight_contract::BlockHash>,
        }

        impl<P: lazy::StateQueryProvider> #struct_name<P> {
            /// Create a new lazy query handle for the given contract address:
            /// a hex string or a typed `ContractAddress`.
            pub fn new(provider: P, address: impl midnight_contract::IntoContractAddress, at_block_hash: Option<midnight_contract::BlockHash>) -> Self {
                Self {
                    address: midnight_contract::IntoContractAddress::into_contract_address_string(address),
                    provider,
                    at_block_hash,
                }
            }

            #(#accessors)*
        }
    }
}

/// Generate a `&[usize]` expression for a field constant's index path.
///
/// For `Single(idx)` the constant is `usize`, so we wrap it: `&[FIELD_X]`.
/// For `Path(p)` the constant is already `&[usize]`.
fn field_indices_expr(const_name: &Ident, field_index: &FieldIndex) -> TokenStream {
    match field_index {
        FieldIndex::Single(_) => quote! { &[#const_name] },
        FieldIndex::Path(_) => quote! { #const_name },
    }
}

fn emit_lazy_field_accessor(field: &LedgerField, const_name: &Ident) -> Option<TokenStream> {
    let method_name = make_ident(&field.name);
    let doc = format!(
        "Query the `{}` ledger field ({}) from the node.",
        field.name, field.storage
    );
    let field_indices = field_indices_expr(const_name, &field.index);
    let path_expr = quote! { lazy::build_query_path(#field_indices) };

    match field.storage {
        StorageKind::Cell => Some(emit_lazy_cell_accessor(
            &method_name,
            &doc,
            &path_expr,
            field.element_type.as_ref(),
        )),
        StorageKind::Counter => Some(emit_lazy_typed_cell_accessor(
            &method_name,
            &doc,
            &path_expr,
            &quote! { u64 },
        )),
        StorageKind::Map => Some(emit_lazy_map_accessor(&method_name, &path_expr, field)),
        StorageKind::Set => Some(emit_lazy_set_accessor(&method_name, &path_expr, field)),
        StorageKind::List => Some(emit_lazy_list_accessor(&method_name, &field_indices, field)),
        // Merkle trees don't support single-value lookup via the RPC.
        StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => None,
    }
}

fn emit_lazy_cell_accessor(
    method_name: &Ident,
    doc: &str,
    path_expr: &TokenStream,
    cell_type: Option<&Type>,
) -> TokenStream {
    match cell_type {
        Some(ty) => emit_lazy_typed_cell_accessor(method_name, doc, path_expr, &type_to_tokens(ty)),
        None => {
            let query_body = lazy_query_body(path_expr);
            quote! {
                #[doc = #doc]
                pub async fn #method_name(&self) -> Result<StateValue<InMemoryDB>, lazy::ContractError> {
                    #query_body
                    Ok(sv)
                }
            }
        }
    }
}

fn emit_lazy_typed_cell_accessor(
    method_name: &Ident,
    doc: &str,
    path_expr: &TokenStream,
    ret_type: &TokenStream,
) -> TokenStream {
    let query_body = lazy_query_body(path_expr);
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self) -> Result<#ret_type, lazy::ContractError> {
            #query_body
            Ok(cell_as::<#ret_type>(&sv)?)
        }
    }
}

fn emit_lazy_map_accessor(
    method_name: &Ident,
    path_expr: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let (key_ty, key_av) = lazy_key(field.key.as_ref());
    let val_ty = storage_arg_tokens(field.value.as_ref());
    let doc = format!("Look up a value by key in the `{}` map (map).", field.name);
    let query = lazy_query_result(&quote! { key_path });
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, key: #key_ty) -> Result<Option<#val_ty>, lazy::ContractError> {
            let mut key_path = #path_expr;
            key_path.push(lazy::value_to_query_key(&#key_av));
            #query
            // No value and no error means key not found
            if result.value.is_none() && result.error.is_none() {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(result)?;
            Ok(Some(cell_as::<#val_ty>(&sv)?))
        }
    }
}

fn emit_lazy_set_accessor(
    method_name: &Ident,
    path_expr: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let (key_ty, key_av) = lazy_key(field.element_type.as_ref());
    let doc = format!("Check if a key exists in the `{}` set (set).", field.name);
    let query = lazy_query_result(&quote! { key_path });
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, key: #key_ty) -> Result<bool, lazy::ContractError> {
            let mut key_path = #path_expr;
            key_path.push(lazy::value_to_query_key(&#key_av));
            #query
            // Sets store Null for present keys; absent keys have no value
            if result.value.is_none() && result.error.is_none() {
                return Ok(false);
            }
            let _sv = lazy::decode_state_value(result)?;
            Ok(true)
        }
    }
}

/// The Rust type of a lazy map key or set element `key`, and the expression
/// that encodes `key` at the layout of `ty`.
fn lazy_key(ty: Option<&Type>) -> (TokenStream, TokenStream) {
    let key = quote! { key };
    let encode = ty.map_or_else(
        || quote! { AlignedValue::from(#key) },
        |ty| encode_to_aligned_value(&key, ty),
    );
    (storage_arg_tokens(ty), encode)
}

fn emit_lazy_list_accessor(
    method_name: &Ident,
    field_indices: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = storage_arg_tokens(field.element_type.as_ref());
    let doc = format!(
        "Get an element by index from the `{}` list (list), counted from the front.\n\n\
         Returns `Ok(None)` if `index` is not less than the list's length. For a large \
         `index`, the read downloads the end of the list (see `lazy::list_element_path`).",
        field.name
    );
    // One call, so that the length and the element come from the same state.
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, index: usize) -> Result<Option<#elem_ty>, lazy::ContractError> {
            let field: &[usize] = #field_indices;
            let lazy::ListElementPath { path, tails_left } = lazy::list_element_path(field, index);
            let results = self.provider.query_contract_state(
                &self.address,
                vec![
                    lazy::StateQuery { path: lazy::list_length_path(field) },
                    lazy::StateQuery { path },
                ],
                self.at_block_hash,
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            let [length, element] = results.as_slice() else {
                return Err(lazy::ContractError::NoValue);
            };
            let sv = lazy::decode_state_value(length)?;
            let len = cell_as::<u64>(&sv)?;
            if index as u64 >= len {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(element)?;
            let value = match tails_left {
                None => cell_as::<#elem_ty>(&sv)?,
                Some(n) => ListAccessor::<#elem_ty>::from_state(&sv)?
                    .get(n)
                    .ok_or(StateError::IndexOutOfBounds(n))??,
            };
            Ok(Some(value))
        }
    }
}

/// Statements that send one `provider.query_contract_state` call for the path
/// `path_expr` and bind its single result as `result`.
fn lazy_query_result(path_expr: &TokenStream) -> TokenStream {
    quote! {
        let results = self.provider.query_contract_state(
            &self.address,
            vec![lazy::StateQuery { path: #path_expr }],
            self.at_block_hash,
        ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
        // One query was sent; a missing result is a malformed RPC response,
        // not a panic.
        let result = results.first().ok_or(lazy::ContractError::NoValue)?;
    }
}

/// Statements that query the path `path_expr` and bind the decoded
/// `StateValue` as `sv`.
fn lazy_query_body(path_expr: &TokenStream) -> TokenStream {
    let query = lazy_query_result(path_expr);
    quote! {
        #query
        let sv = lazy::decode_state_value(result)?;
    }
}

// ---------------------------------------------------------------------------
// Circuits struct — async on-chain call methods
// ---------------------------------------------------------------------------

fn emit_circuits_struct(info: &crate::types::ContractInfo, ledger_name: &Ident) -> TokenStream {
    // Per circuit we emit a constructor on `Circuits` (returns a call builder)
    // plus a standalone call struct with `build()`, `send()`, `simulate()` and
    // an `IntoFuture` impl.
    let mut constructors = Vec::new();
    let mut call_items = Vec::new();
    let send_bounds = quote! {
        P: midnight_contract::AsMidnightProvider
            + ::core::marker::Send
            + ::core::marker::Sync,
        Wp: midnight_contract::runtime::WitnessProvider
            + ::core::marker::Send
            + ::core::marker::Sync,
    };

    for circuit in &info.circuits {
        // A pure circuit is evaluated off-chain and has no call path.
        if circuit.pure() {
            continue;
        }

        let method_name = make_ident(&circuit.name);
        let call_ty = format_ident!("{}CallBuilder", to_pascal_case(&circuit.name));
        let circuit_name_str = &circuit.name;

        let doc = format!(
            "Start a call to the `{}` circuit.\n\n\
             Returns a call builder. `.await` it to submit the call, wait for finality \
             and get the circuit's result. `.send().await` submits the call and returns \
             a `PendingCall` before the chain's verdict. `.build().await` builds and \
             proves the transaction and returns its bytes *without* a submit, to merge \
             with other transactions. `.simulate().await` runs the circuit with no \
             proof, fee or transaction.",
            circuit.name
        );
        let call_doc = format!(
            "Call builder for the `{}` circuit.\n\n\
             `.await` submits the call, waits for finality and returns the circuit's \
             result. [`send`](Self::send) submits the call and returns a `PendingCall` \
             before the chain's verdict. [`build`](Self::build) returns the proven \
             transaction bytes without a submit. [`simulate`](Self::simulate) runs the \
             circuit and returns its value and the ledger after it, with no proof, fee \
             or transaction.",
            circuit.name
        );

        let (value_ty, decode) = circuit_decode(circuit);

        // Submit path (`.await` via `IntoFuture`): call + return the circuit's
        // typed result. `__circuits` is destructured from `self` at the call
        // site below; `ir`, `program` and `__args` come from the shared `setup`.
        let call_args = quote! {
            ir, &program, #circuit_name_str, &__args, &__circuits.witnesses, &__circuits.coin_encryption_keys, ::core::mem::take(&mut __circuits.shielded)
        };
        let ret_type = quote! {
            ::core::result::Result<midnight_contract::CallOutcome<#value_ty>, midnight_contract::ContractError>
        };
        let submit_tail = quote! {
            #decode
            let __outcome = __circuits.contract.call_with(#call_args).await?;
            ::core::result::Result::Ok(midnight_contract::CallOutcome {
                value: __decode(__outcome.value)?,
                extrinsic_hash: __outcome.extrinsic_hash,
                transaction_hash: __outcome.transaction_hash,
                block_hash: __outcome.block_hash,
            })
        };

        // Constructor params / call-struct fields (identical syntax) and the
        // destructure idents, each with a leading comma.
        let param_list = typed_params(circuit.arguments());
        let ident_list: Vec<_> = circuit
            .arguments()
            .iter()
            .map(|arg| make_ident(arg.name.name()))
            .collect();
        let params = quote! { #(, #param_list)* };
        let field_idents = quote! { #(, #ident_list)* };
        let setup = circuit_setup(ledger_name, circuit);
        let open = quote! {
            let #call_ty { circuits: __circuits #field_idents } = self;
            #setup
        };

        // Constructor on `Circuits`: returns the call builder holding a `&mut`
        // to this `Circuits` (so the next call's shielded inputs / witnesses are
        // read at execution time).
        constructors.push(quote! {
            #[doc = #doc]
            pub fn #method_name(&mut self #params) -> #call_ty<'_, 'a, P, Wp> {
                #call_ty { circuits: self #field_idents }
            }
        });

        // The call builder: `.await` (via `IntoFuture`) submits and returns the
        // circuit's result; `.send().await` returns the pending call;
        // `.build().await` returns the proven bytes; `.simulate().await`
        // returns the value and the post-call ledger.
        call_items.push(quote! {
            #[doc = #call_doc]
            #[must_use = "does nothing until awaited, sent, built or simulated"]
            pub struct #call_ty<'c, 'a, P, Wp = midnight_contract::runtime::NoWitnesses> {
                circuits: &'c mut Circuits<'a, P, Wp>
                #params
            }

            impl<'c, 'a, P, Wp> #call_ty<'c, 'a, P, Wp>
            where
                P: midnight_contract::AsMidnightProvider,
                Wp: midnight_contract::runtime::WitnessProvider,
            {
                /// Build and prove the call transaction and return its bytes,
                /// **without** submitting. Combine it with other transactions via
                /// `MidnightProvider::merge_transactions`, then submit yourself.
                ///
                /// The build records no private-state snapshot. So it fails with
                /// `ContractError::UnrecordedPrivateState` when the witnesses change the
                /// private state, after the local run and before the build reserves an
                /// input or makes a proof. Submit such a call with `.await` or `.send()`.
                ///
                /// The build reserves the wallet inputs that the transaction spends. The
                /// bytes do not name them, so they stay reserved until a sync sees the chain
                /// spend them, or until their TTL passes.
                /// See `midnight_contract::Contract::build_call_with`.
                pub async fn build(
                    self,
                ) -> ::core::result::Result<::std::vec::Vec<u8>, midnight_contract::ContractError> {
                    // Boxed for the same reason `without_dust` below is.
                    ::std::boxed::Box::pin(async move {
                        #open
                        let __bytes = __circuits.contract.build_call_with(#call_args, true).await?;
                        ::core::result::Result::Ok(__bytes)
                    })
                    .await
                }

                /// Submit the call, and return a `midnight_contract::PendingCall` before
                /// the chain's verdict.
                ///
                /// Read the transaction's hashes from the handle. Then call its
                /// `wait_finalized` to settle the private state and get the `CallOutcome`.
                /// See `midnight_contract::Contract::send_call_with`.
                pub async fn send(
                    self,
                ) -> ::core::result::Result<
                    midnight_contract::PendingCall<#value_ty>,
                    midnight_contract::ContractError,
                > {
                    // Boxed for the same reason `without_dust` below is.
                    ::std::boxed::Box::pin(async move {
                        #open
                        #decode
                        __circuits.contract.send_call_with(#call_args, __decode).await
                    })
                    .await
                }

                /// Run the call locally, with no proof, fee or transaction.
                ///
                /// It returns the circuit's value and the ledger after the call.
                /// It reads the state and the block time at the node's best block, or at
                /// the `at_block` pin of the handle, and runs the circuit there. The
                /// provider needs a wallet only for a circuit that calls `ownPublicKey()`.
                /// The attached shielded inputs stay on `Circuits` for the next call. See
                /// `midnight_contract::Contract::simulate_with`.
                pub async fn simulate(
                    self,
                ) -> ::core::result::Result<
                    midnight_contract::Simulated<#value_ty, #ledger_name>,
                    midnight_contract::ContractError,
                > {
                    // Boxed for the same reason `without_dust` below is.
                    ::std::boxed::Box::pin(async move {
                        #open
                        #decode
                        let __simulated = __circuits.contract.simulate_with(ir, &program, &__args, &__circuits.witnesses).await?;
                        ::core::result::Result::Ok(midnight_contract::Simulated {
                            value: __decode(__simulated.value)?,
                            ledger: #ledger_name::new(__simulated.ledger),
                            block_hash: __simulated.block_hash,
                        })
                    })
                    .await
                }
                // `.without_dust()` comes from the `midnight_contract::DustlessBuilder`
                // trait impl below (bring the trait into scope to call it).
            }

            // Build + prove the **Dustless** (fee-less) call, for another wallet
            // to sponsor via `MidnightProvider::balance_transaction`.
            impl<'c, 'a, P, Wp> midnight_contract::DustlessBuilder for #call_ty<'c, 'a, P, Wp>
            where
                #send_bounds
            {
                type Error = midnight_contract::ContractError;
                // Boxed for the same reason the `IntoFuture` impl below is: the
                // build-and-prove future is large, and leaving it inline here
                // makes the caller's frame big enough to overflow the stack on
                // debug builds.
                /// Build and prove the call with no Dust fee, for another wallet to
                /// sponsor with `MidnightProvider::balance_transaction`.
                ///
                /// As with `build`, it fails with `ContractError::UnrecordedPrivateState`
                /// when the witnesses change the private state, before the build reserves
                /// an input or makes a proof. Submit such a call with `.await` or `.send()`.
                /// As with `build`, the inputs that the build reserves stay reserved until
                /// a sync sees the chain spend them, or until their TTL passes.
                fn without_dust(
                    self,
                ) -> impl ::core::future::Future<
                    Output = ::core::result::Result<
                        midnight_contract::DustlessTransaction,
                        midnight_contract::ContractError,
                    >,
                > + ::core::marker::Send {
                    ::std::boxed::Box::pin(async move {
                        #open
                        let __bytes = __circuits.contract.build_call_with(#call_args, false).await?;
                        ::core::result::Result::Ok(midnight_contract::DustlessTransaction::from_proven_bytes(__bytes))
                    })
                }
            }

            impl<'c, 'a, P, Wp> ::core::future::IntoFuture for #call_ty<'c, 'a, P, Wp>
            where
                #send_bounds
            {
                type Output = #ret_type;
                type IntoFuture = ::core::pin::Pin<
                    ::std::boxed::Box<
                        dyn ::core::future::Future<Output = #ret_type> + ::core::marker::Send + 'c,
                    >,
                >;
                fn into_future(self) -> Self::IntoFuture {
                    ::std::boxed::Box::pin(async move {
                        #open
                        #submit_tail
                    })
                }
            }
        });
    }

    // The typed `with_witnesses` only exists for contracts that declare
    // witnesses (otherwise the `Witnesses` / `WitnessesAdapter` types aren't
    // generated). Circuits stays generic-with-default so witness-free contracts
    // still call through `NoWitnesses`.
    let with_witnesses_impl = if info.witnesses.is_empty() {
        quote! {}
    } else {
        quote! {
            impl<'a, P> Circuits<'a, P, midnight_contract::runtime::NoWitnesses> {
                /// Attach a typed [`Witnesses`] implementation. Each circuit call
                /// then loads the contract's private state before execution,
                /// threads it through the witnesses, and persists it after.
                ///
                /// Only a submitted call (`.await` or `.send()`) records the state.
                /// A call builder's `build` and `without_dust` refuse a call whose
                /// witnesses change the state, with
                /// `ContractError::UnrecordedPrivateState`. [`Witnesses`] says when
                /// the state counts as changed.
                pub fn with_witnesses<W: Witnesses>(
                    self,
                    witnesses: &'a W,
                ) -> Circuits<'a, P, WitnessesAdapter<'a, W>> {
                    Circuits {
                        contract: self.contract,
                        witnesses: WitnessesAdapter(witnesses),
                        coin_encryption_keys: self.coin_encryption_keys,
                        shielded: self.shielded,
                    }
                }
            }
        }
    };

    quote! {
        /// On-chain circuit call methods.
        ///
        /// Access via `contract.circuits()`. Each method executes the circuit
        /// locally, builds a funded transaction, and submits it to the node.
        /// The `simulate` method of a call builder stops after the local run.
        /// For circuits that call witnesses, chain [`Circuits::with_witnesses`]
        /// with a typed [`Witnesses`] impl. When a `PrivateStateProvider` is
        /// attached, the contract's private state is threaded automatically,
        /// keyed by the contract address.
        pub struct Circuits<'a, P, Wp = midnight_contract::runtime::NoWitnesses> {
            contract: &'a midnight_contract::Contract<P>,
            witnesses: Wp,
            coin_encryption_keys: Vec<midnight_contract::ShieldedRecipient>,
            shielded: midnight_contract::ShieldedInputs,
        }

        #with_witnesses_impl

        impl<'a, P, Wp> Circuits<'a, P, Wp> {
            /// Attach wallet coins to spend as shielded (Zswap) inputs for the
            /// next circuit call, funding a circuit that receives a shielded
            /// coin (e.g. `receiveShielded`). Enumerate the wallet's coins with
            /// `MidnightProvider::spendable_shielded_coins`.
            ///
            /// Each coin is selected exactly by its nullifier and routed to the
            /// segment of the output it funds. The attached coins must COVER
            /// the value the circuit receives: Zswap balances per (token,
            /// segment) delta, not per coin identity, so one coin may fund a
            /// `receiveShielded` of its own value, or several coins may fund
            /// one larger receive. Whatever they carry beyond what the
            /// circuit's outputs draw returns to this wallet as a change
            /// output. Coins that fall short leave the call unbalanced, which
            /// the fee-paying step refuses before submitting; a Dustless call
            /// carries the shortfall to the node instead.
            ///
            /// Every coin must be attached in a single call, since this replaces
            /// the set rather than extending it. The attached inputs apply to
            /// the next call on this builder.
            pub fn with_shielded_inputs(
                mut self,
                coins: impl IntoIterator<Item = midnight_contract::SpendableShieldedCoin>,
            ) -> Self {
                self.shielded.coins = coins.into_iter().collect();
                self
            }


            /// Supply the encryption key of each recipient of the shielded coins
            /// these circuit calls create (e.g. via `mintShieldedToken`). The
            /// SDK seals a discovery ciphertext to that key on each output whose
            /// coin public key matches a recipient. The recipient's wallet then
            /// finds the coin through normal sync, with no `watchFor`. Without
            /// an entry, an external recipient has to scan for the coin
            /// explicitly. `parse_shielded_recipient` reads a recipient from a
            /// shielded address.
            ///
            /// The calling wallet's own coin public key needs no entry: the SDK
            /// seals a coin sent to `ownPublicKey()` to the wallet's encryption
            /// key. An entry for that key takes priority.
            pub fn with_coin_encryption_keys(
                mut self,
                coin_encryption_keys: impl IntoIterator<Item = midnight_contract::ShieldedRecipient>,
            ) -> Self {
                self.coin_encryption_keys = coin_encryption_keys.into_iter().collect();
                self
            }
        }

        impl<'a, P, Wp> Circuits<'a, P, Wp>
        where
            P: midnight_contract::AsMidnightProvider,
            Wp: midnight_contract::runtime::WitnessProvider,
        {
            #(#constructors)*
        }

        #(#call_items)*
    }
}

/// Emit `Simulator`, a typed wrapper over `midnight_contract::local::Runner`
/// with one method per impure circuit.
pub(crate) fn emit_simulator(
    info: &crate::types::ContractInfo,
    contract_name: &str,
) -> TokenStream {
    let ledger_name = format_ident!("{}", contract_name);
    let methods: Vec<_> = info
        .circuits
        .iter()
        .filter(|circuit| !circuit.pure())
        .map(|circuit| {
            let method_name = make_ident(&circuit.name);
            let params = typed_params(circuit.arguments());
            let return_ty = return_type_ident(&circuit.name);
            let setup = circuit_setup(&ledger_name, circuit);
            let (_, decode) = circuit_decode(circuit);
            let doc = format!(
                "Run the `{}` circuit in process.\n\n\
                 On success, the simulator keeps the ledger and the private state after \
                 the call.\n\n\
                 # Errors\n\n\
                 `ContractError::Interpreter` when the circuit or a witness fails, for \
                 example on a failed `assert`. The simulator then keeps the state from \
                 before the call.",
                circuit.name
            );
            quote! {
                #[doc = #doc]
                pub fn #method_name(
                    &mut self
                    #(, #params)*
                ) -> ::core::result::Result<#return_ty, midnight_contract::ContractError> {
                    #setup
                    #decode
                    __decode(self.0.run(ir, &program, &__args)?)
                }
            }
        })
        .collect();

    // `Witnesses` and `WitnessesAdapter` exist only for a contract that
    // declares witnesses.
    let with_witnesses = if info.witnesses.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            impl Simulator<midnight_contract::runtime::NoWitnesses> {
                /// Answer the circuits' witness calls with a typed [`Witnesses`]
                /// impl. The private state starts at its `Default`, and each
                /// successful call keeps the private state that its witnesses left.
                pub fn with_witnesses<'w, W: Witnesses>(
                    self,
                    witnesses: &'w W,
                ) -> Simulator<WitnessesAdapter<'w, W>> {
                    Simulator(self.0.with_witnesses(WitnessesAdapter(witnesses)))
                }
            }
        }
    };

    quote! {
        /// Runs the contract's circuits in process, with no chain, proof or
        /// wallet.
        ///
        /// Each circuit method starts from the ledger and the private state
        /// that the last successful call left, and a failed call changes
        /// nothing. The simulator runs circuits only: it makes no Zswap offer,
        /// and it has no Dust and no fees.
        pub struct Simulator<Wp = midnight_contract::runtime::NoWitnesses>(
            midnight_contract::local::Runner<Wp>,
        );

        impl Simulator {
            /// A simulator over `state` at `block_time`, at the zero address,
            /// with no coin public key.
            ///
            /// The block time floors to whole seconds, the unit of the chain's
            /// clock checks.
            ///
            /// # Panics
            ///
            /// When `block_time` is before the Unix epoch.
            pub fn new(
                state: impl ::core::convert::Into<ContractState<InMemoryDB>>,
                block_time: ::std::time::SystemTime,
            ) -> Self {
                Self(midnight_contract::local::Runner::new(state.into(), block_time))
            }
        }

        #with_witnesses

        impl<Wp> Simulator<Wp> {
            /// Set the time that the circuits' clock checks compare against. It
            /// floors to whole seconds.
            ///
            /// # Panics
            ///
            /// When `time` is before the Unix epoch.
            pub fn set_block_time(&mut self, time: ::std::time::SystemTime) {
                self.0.set_block_time(time);
            }

            /// Set the address that `kernel.self()` reads.
            pub fn set_address(&mut self, address: midnight_contract::ContractAddress) {
                self.0.set_address(address);
            }

            /// Set the caller's coin public key, which `ownPublicKey()` returns.
            pub fn set_coin_public_key(&mut self, key: midnight_contract::CoinPublicKey) {
                self.0.set_coin_public_key(key);
            }

            /// The ledger after the last successful call.
            pub fn ledger(&self) -> #ledger_name {
                #ledger_name::new(::core::clone::Clone::clone(self.0.state()))
            }
        }

        impl<Wp> ::core::fmt::Debug for Simulator<Wp> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_tuple("Simulator").field(&self.0).finish()
            }
        }

        impl<Wp: midnight_contract::runtime::WitnessProvider> Simulator<Wp> {
            #(#methods)*
        }
    }
}

/// The statements that run before generated code executes `circuit`.
///
/// They build the program that the circuit resolves its calls against (typed
/// constructors, checked by the compiler) as `program`, resolve the circuit in
/// it as `ir`, and bind the arguments as `__args`. Each argument must be in
/// scope as a local of its own name.
pub(super) fn circuit_setup(ledger_name: &Ident, circuit: &Circuit) -> TokenStream {
    let circuit_id = circuit.def.name.0.as_str();
    let args_expr = if circuit.arguments().is_empty() {
        quote! { let __args: [(&str, midnight_contract::runtime::Value); 0] = []; }
    } else {
        let binding_list: Vec<_> = circuit
            .arguments()
            .iter()
            .map(|arg| {
                let name_str = arg.name.name();
                let name_ident = make_ident(name_str);
                let conversion =
                    super::circuit_calls::type_to_value_conversion(&name_ident, &arg.ty);
                quote! { (#name_str, #conversion) }
            })
            .collect();
        quote! { let __args = [#(#binding_list),*]; }
    };

    quote! {
        let helpers = #ledger_name::__helpers();
        let witnesses = #ledger_name::__witnesses();
        let natives = #ledger_name::__natives();
        let program = midnight_contract::interpreter::Program::new(
            &helpers,
            &witnesses,
            &natives,
        );
        // The circuit travels once, in `__helpers()`; resolve it there
        // rather than embedding a second copy per call site.
        let ir = program.circuit(#circuit_id).ok_or_else(|| {
            midnight_contract::ContractError::Serialization(::std::format!(
                "the embedded program declares no circuit `{}`", #circuit_id
            ))
        })?;
        #args_expr
    }
}

/// The type of the value of `circuit`, and `fn __decode`, which turns the
/// circuit's raw result into that value.
///
/// Every generated path that reads the circuit's result defines and calls
/// `__decode`, so all of them read it the same way.
pub(super) fn circuit_decode(circuit: &Circuit) -> (TokenStream, TokenStream) {
    if super::circuit_calls::is_void_type(circuit.result_type()) {
        let decode = quote! {
            fn __decode(
                _: midnight_contract::runtime::Value,
            ) -> ::core::result::Result<(), midnight_contract::ContractError> {
                ::core::result::Result::Ok(())
            }
        };
        return (quote! { () }, decode);
    }

    let value_ty = type_to_tokens(circuit.result_type());
    let conversion = super::circuit_calls::value_to_type_conversion(
        circuit.result_type(),
        &format!("circuit `{}` return value", circuit.name),
    );
    let decode = quote! {
        fn __decode(
            __val: midnight_contract::runtime::Value,
        ) -> ::core::result::Result<#value_ty, midnight_contract::ContractError> {
            // The conversion evaluates to Result<_, InterpreterError>,
            // which `From`-converts into ContractError.
            ::core::result::Result::Ok((#conversion)?)
        }
    };
    (value_ty, decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter_field(name: &str, index: FieldIndex) -> LedgerField {
        LedgerField {
            name: name.to_string(),
            index,
            storage: StorageKind::Counter,
            exported: true,
            element_type: None,
            key: None,
            value: None,
            depth: None,
        }
    }

    /// The state expression `build()` hands to `ContractState::new`.
    fn built_state(fields: &[LedgerField]) -> String {
        let out = emit_initial_state(fields, "Wide", false)
            .to_string()
            .replace(' ', "");
        out.split("ContractState::new(")
            .nth(1)
            .and_then(|tail| tail.split(",StorageHashMap::new()").next())
            .expect("build() constructs a ContractState")
            .to_string()
    }

    #[test]
    fn an_empty_ledger_builds_an_empty_array() {
        assert_eq!(built_state(&[]), "StateValue::Array(vec![].into())");
    }

    /// A ledger that fits one array keeps the array it always built — the
    /// only contracts that worked before this are single-level ones.
    #[test]
    fn a_single_level_ledger_builds_one_flat_array() {
        // Declared out of index order: the array follows the artifact's
        // indices, which is what the circuits read.
        let fields = vec![
            counter_field("second", FieldIndex::Single(1)),
            counter_field("first", FieldIndex::Single(0)),
            counter_field("third", FieldIndex::Single(2)),
        ];

        let state = built_state(&fields);

        assert_eq!(
            state.matches("StateValue::Array(vec![").count(),
            1,
            "a single-level ledger must stay flat, got: {state}"
        );
        assert!(
            state.starts_with(
                "StateValue::Array(vec![StateValue::from(self.first),StateValue::from(self.second),StateValue::from(self.third)]"
            ),
            "expected the cells in index order, got: {state}"
        );
    }
}
