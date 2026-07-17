use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::types::{FieldIndex, LedgerField, StorageKind, TypeNode};

use super::helpers::{Lit, make_ident};
use super::types::type_to_tokens;

pub(crate) fn emit_ledger_wrapper(
    fields: &[LedgerField],
    name: &str,
    ir_constants: &TokenStream,
    info: &crate::types::ContractInfo,
) -> TokenStream {
    let struct_name = format_ident!("{}", name);
    let query_struct_name = format_ident!("{}Query", name);

    let accessors: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let field_index = field.field_index()?;
            let const_name = format_ident!("FIELD_{}", field.name.to_uppercase());
            Some(emit_field_accessor(field, &const_name, &field_index))
        })
        .collect();

    // Pure functions are inlined by the compiler — no __HELPERS_JSON needed.

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
    let initial_state = emit_initial_state(fields, name);

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

            /// Create from a hex-encoded contract state string (as returned by the indexer).
            pub fn from_hex(hex_state: &str) -> Result<Self, StateError> {
                let bytes = hex::decode(hex_state).map_err(|e| StateError::HexDecode(e.to_string()))?;
                let state: ContractState<InMemoryDB> = tagged_deserialize(&mut &bytes[..]).map_err(StateError::Deserialize)?;
                Ok(Self::new(state))
            }

            /// Fetch the current contract state from a provider and wrap it.
            pub async fn from_provider<P: midnight_contract::Provider>(
                provider: &P,
                address: &str,
            ) -> Result<Self, midnight_contract::ContractError> {
                let state = midnight_contract::state::fetch_state(provider, address).await?;
                Ok(Self::new(state))
            }

            #state_accessor

            #(#accessors)*

            #ir_constants
        }

        impl midnight_contract::FromHex for #struct_name {
            fn from_hex(hex_state: &str) -> Result<Self, StateError> {
                #struct_name::from_hex(hex_state)
            }
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
            pub fn deploy<P>(provider: P) -> DeployBuilder<P>
            where
                P: midnight_contract::AsMidnightProvider + midnight_contract::Provider,
            {
                DeployBuilder(midnight_contract::Contract::deploy(provider))
            }

            /// Create a handle for an already-deployed contract at the given
            /// address: a hex string or a typed `ContractAddress`.
            ///
            /// This is synchronous, no network calls are made. Call `.build()`
            /// on the returned builder to get the `Contract<P>` handle.
            pub fn at<P>(
                provider: P,
                address: impl midnight_contract::IntoAddress,
            ) -> ConnectBuilder<P>
            where
                P: midnight_contract::AsMidnightProvider + midnight_contract::Provider,
            {
                ConnectBuilder(midnight_contract::Contract::at(provider, address))
            }
        }

        /// Builder wrapper around `midnight_contract::DeployBuilder` that
        /// yields the generated `Contract<P>` on deploy.
        pub struct DeployBuilder<P>(midnight_contract::DeployBuilder<P>);

        impl<P> DeployBuilder<P> {
            /// Set the initial contract state.
            pub fn with_initial_state(self, state: impl Into<ContractState<InMemoryDB>>) -> Self {
                Self(self.0.with_initial_state(state))
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

            /// Set the timeout for waiting for deployment confirmation.
            pub fn with_deploy_timeout(self, timeout: std::time::Duration) -> Self {
                Self(self.0.with_deploy_timeout(timeout))
            }

            /// Set the poll interval for checking deployment status.
            pub fn with_deploy_poll_interval(self, interval: std::time::Duration) -> Self {
                Self(self.0.with_deploy_poll_interval(interval))
            }

            /// Submit the deploy transaction and return a `PendingDeploy` handle.
            ///
            /// Use [`PendingDeploy::wait_best`] / [`PendingDeploy::wait_finalized`]
            /// to observe inclusion states, then [`PendingDeploy::into_contract`]
            /// to wait for the indexer and obtain the typed `Contract<P>`.
            pub async fn send(self) -> Result<PendingDeploy<P>, midnight_contract::ContractError>
            where
                P: midnight_contract::AsMidnightProvider + midnight_contract::Provider + Send,
            {
                Ok(PendingDeploy(self.0.send().await?))
            }
        }

        impl<P> std::future::IntoFuture for DeployBuilder<P>
        where
            P: midnight_contract::AsMidnightProvider + midnight_contract::Provider + Send + 'static,
        {
            type Output = Result<Contract<P>, midnight_contract::ContractError>;
            type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

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
            pub fn extrinsic_hash(&self) -> [u8; 32] {
                self.0.extrinsic_hash()
            }

            /// The extrinsic hash formatted as a hex string (no `0x` prefix).
            pub fn extrinsic_hash_hex(&self) -> String {
                self.0.extrinsic_hash_hex()
            }

            /// Wait until the deploy transaction lands in the best block.
            ///
            /// Consumes `self` and returns it back so callers can chain.
            pub async fn wait_best(
                self,
            ) -> Result<(midnight_contract::TxInBlock, Self), midnight_contract::ContractError> {
                let (in_block, inner) = self.0.wait_best().await?;
                Ok((in_block, Self(inner)))
            }

            /// Wait until the deploy transaction is in a finalized block.
            ///
            /// Consumes `self` and returns it back. May be called without a
            /// prior `wait_best`; the best-block status is then skipped.
            pub async fn wait_finalized(
                self,
            ) -> Result<(midnight_contract::TxInBlock, Self), midnight_contract::ContractError> {
                let (in_block, inner) = self.0.wait_finalized().await?;
                Ok((in_block, Self(inner)))
            }
        }

        impl<P> PendingDeploy<P>
        where
            P: midnight_contract::AsMidnightProvider + midnight_contract::Provider + Send,
        {
            /// Wait for the indexer and return the typed `Contract<P>`.
            pub async fn into_contract(self) -> Result<Contract<P>, midnight_contract::ContractError> {
                self.0.into_contract().await.map(Contract)
            }
        }

        /// Builder wrapper around `midnight_contract::ConnectBuilder` that
        /// yields the generated `Contract<P>` on build.
        pub struct ConnectBuilder<P>(midnight_contract::ConnectBuilder<P>);

        impl<P> ConnectBuilder<P> {
            /// Set the source of the contract's compiled ZK artifacts (a directory
            /// path or a custom `ZkConfigProvider`); see `midnight_contract::IntoZkConfig`.
            pub fn with_zk_config(self, zk_config: impl midnight_contract::IntoZkConfig) -> Self {
                Self(self.0.with_zk_config(zk_config))
            }

            /// Pin queries to the block `hash`. Default is latest.
            pub fn at_block(self, hash: midnight_contract::NodeBlockHash) -> Self {
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

        impl<P: midnight_contract::Provider> Contract<P> {
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
            P: midnight_contract::AsMidnightProvider + midnight_contract::Provider,
        {
            /// Fetch the current ledger state from the node.
            ///
            /// Returns the sync `Ledger` struct with typed field accessors.
            /// Uses the `midnight_contractState` node RPC which is available
            /// on all standard devnet nodes.
            pub async fn ledger(&self) -> Result<#struct_name, midnight_contract::ContractError> {
                let provider = self.0.provider().as_midnight_provider();
                let state = midnight_contract::state::fetch_state_from_node(
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
            P: midnight_contract::Provider,
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

/// Generate a `get_field` or `get_field_path` call depending on the index type.
fn navigate_to_field(const_name: &Ident, field_index: &FieldIndex) -> TokenStream {
    match field_index {
        FieldIndex::Single(_) => {
            quote! { get_field(self.state.data.get_ref(), #const_name) }
        }
        FieldIndex::Path(_) => {
            quote! { get_field_path(self.state.data.get_ref(), #const_name) }
        }
    }
}

fn emit_field_accessor(
    field: &LedgerField,
    const_name: &Ident,
    field_index: &FieldIndex,
) -> TokenStream {
    let method_name = make_ident(&field.name);
    let nav = navigate_to_field(const_name, field_index);
    let doc = format!(
        "Access the `{}` ledger field ({}).",
        field.name, field.storage
    );

    match field.storage {
        StorageKind::Cell => {
            emit_cell_accessor(&method_name, &doc, &nav, field.element_type.as_ref())
        }
        StorageKind::Counter => emit_counter_accessor(&method_name, &doc, &nav),
        StorageKind::Map => emit_map_accessor(&method_name, &doc, &nav, field),
        StorageKind::Set => emit_set_accessor(&method_name, &doc, &nav, field),
        StorageKind::List => emit_list_accessor(&method_name, &doc, &nav, field),
        StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => {
            emit_merkle_tree_accessor(&method_name, &doc, &nav)
        }
    }
}

fn emit_cell_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    cell_type: Option<&TypeNode>,
) -> TokenStream {
    if let Some(ty) = cell_type {
        let (ret_type, body) = cell_accessor(ty, nav);
        quote! {
            #[doc = #doc]
            pub fn #method_name(&self) -> Result<#ret_type, StateError> {
                #body
            }
        }
    } else {
        quote! {
            #[doc = #doc]
            pub fn #method_name(&self) -> Result<&StateValue<InMemoryDB>, StateError> {
                #nav
            }
        }
    }
}

fn emit_counter_accessor(method_name: &Ident, doc: &str, nav: &TokenStream) -> TokenStream {
    let body = cell_value_body(&quote! { u64 }, nav);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<u64, StateError> {
            #body
        }
    }
}

fn emit_map_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let key_ty = field
        .key
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    let val_ty = field
        .value
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<MapAccessor<'_, #key_ty, #val_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Map(map) => Ok(MapAccessor::new(map)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Map",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_set_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = field
        .element_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<SetAccessor<'_, #elem_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Map(map) => Ok(SetAccessor::new(map)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Map",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_list_accessor(
    method_name: &Ident,
    doc: &str,
    nav: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = field
        .element_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<ListAccessor<'_, #elem_ty>, StateError> {
            let sv = #nav?;
            match sv {
                StateValue::Array(arr) => Ok(ListAccessor::new(arr)),
                _ => Err(StateError::UnexpectedVariant {
                    expected: "Array",
                    actual: variant_name(sv),
                }),
            }
        }
    }
}

fn emit_merkle_tree_accessor(method_name: &Ident, doc: &str, nav: &TokenStream) -> TokenStream {
    quote! {
        #[doc = #doc]
        pub fn #method_name(&self) -> Result<MerkleTreeAccessor<'_>, StateError> {
            let sv = #nav?;
            MerkleTreeAccessor::from_state(sv)
        }
    }
}

// ---------------------------------------------------------------------------
// InitialState: typed struct for contract deployment
// ---------------------------------------------------------------------------

fn emit_initial_state(fields: &[LedgerField], name: &str) -> TokenStream {
    let struct_name = format_ident!("{}InitialState", name);
    let ledger_name = format_ident!("{}", name);

    if fields.is_empty() {
        return quote! {
            /// Initial state for deploying this contract.
            #[derive(Debug, Clone, Default)]
            pub struct #struct_name;

            impl #struct_name {
                /// Build the `ContractState` for deployment.
                pub fn build(self) -> ContractState<InMemoryDB> {
                    ContractState::new(
                        StateValue::Array(vec![].into()),
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
        };
    }

    let mut field_defs = Vec::new();
    let mut field_defaults = Vec::new();
    let mut field_conversions = Vec::new();

    for field in fields {
        let field_name = make_ident(&field.name);
        let doc = format!("Initial value for `{}`.", field.name);

        match field.storage {
            StorageKind::Cell => {
                // Use typed fields only for simple scalar types that have
                // Default + Into<AlignedValue>. Complex types use AlignedValue.
                let is_simple = matches!(
                    &field.element_type,
                    Some(TypeNode::Uint { .. }) | Some(TypeNode::Boolean)
                );
                if is_simple {
                    let rust_type = type_to_tokens(field.element_type.as_ref().unwrap());
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: #rust_type });
                    field_defaults.push(quote! { #field_name: Default::default() });
                    field_conversions
                        .push(quote! { StateValue::from(AlignedValue::from(self.#field_name)) });
                } else {
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: AlignedValue });
                    // An unset cell defaults to its type's zero value, not the
                    // unit value: a `Bytes<N>` cell reads back with `Bytes<N>`
                    // alignment, so a null default diverges from the circuit's
                    // typed read at proof time. Give `Bytes<N>` a zero-filled
                    // value; other complex cells keep the unit fallback.
                    let default_value = match &field.element_type {
                        Some(TypeNode::Bytes { length }) => {
                            let len = Lit(*length);
                            quote! { AlignedValue::from(Bytes([0u8; #len])) }
                        }
                        _ => quote! { AlignedValue::from(()) },
                    };
                    field_defaults.push(quote! { #field_name: #default_value });
                    field_conversions.push(quote! { StateValue::from(self.#field_name.clone()) });
                }
            }
            StorageKind::Counter => {
                field_defs.push(quote! { #[doc = #doc] pub #field_name: u64 });
                field_defaults.push(quote! { #field_name: 0 });
                field_conversions.push(quote! { StateValue::from(self.#field_name) });
            }
            StorageKind::Map | StorageKind::Set => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StorageHashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StorageHashMap::new() });
                field_conversions.push(quote! { StateValue::Map(self.#field_name) });
            }
            StorageKind::List => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StateValue::Array(StorageArray::new()) });
                field_conversions.push(quote! { self.#field_name });
            }
            StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StateValue::Null });
                field_conversions.push(quote! { self.#field_name });
            }
        }
    }

    quote! {
        /// Initial state for deploying this contract.
        #[derive(Debug, Clone)]
        pub struct #struct_name {
            #(#field_defs),*
        }

        impl Default for #struct_name {
            fn default() -> Self {
                Self {
                    #(#field_defaults),*
                }
            }
        }

        impl #struct_name {
            /// Build the `ContractState` for deployment.
            pub fn build(self) -> ContractState<InMemoryDB> {
                ContractState::new(
                    StateValue::Array(
                        vec![#(#field_conversions),*].into(),
                    ),
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

// ---------------------------------------------------------------------------
// Lazy wrapper (query-per-field via Provider)
// ---------------------------------------------------------------------------

pub(crate) fn emit_lazy_ledger_wrapper(fields: &[LedgerField], name: &str) -> TokenStream {
    let struct_name = format_ident!("{}Query", name);

    let accessors: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let field_index = field.field_index()?;
            let const_name = format_ident!("FIELD_{}", field.name.to_uppercase());
            emit_lazy_field_accessor(field, &const_name, &field_index)
        })
        .collect();

    quote! {
        /// Lazy query interface — each accessor calls the RPC to fetch only
        /// the requested field instead of downloading the full contract state.
        pub struct #struct_name<P: lazy::StateQueryProvider> {
            address: String,
            provider: P,
            at_block_hash: Option<midnight_contract::NodeBlockHash>,
        }

        impl<P: lazy::StateQueryProvider> #struct_name<P> {
            /// Create a new lazy query handle for the given contract address:
            /// a hex string or a typed `ContractAddress`.
            pub fn new(provider: P, address: impl midnight_contract::IntoAddress, at_block_hash: Option<midnight_contract::NodeBlockHash>) -> Self {
                Self {
                    address: midnight_contract::IntoAddress::into_address_string(address),
                    provider,
                    at_block_hash,
                }
            }

            #(#accessors)*
        }
    }
}

/// Generate the query path expression for a field constant.
///
/// For `Single(idx)` the constant is `usize`, so we wrap it: `&[FIELD_X]`.
/// For `Path(p)` the constant is already `&[usize]`.
fn query_path_expr(const_name: &Ident, field_index: &FieldIndex) -> TokenStream {
    match field_index {
        FieldIndex::Single(_) => quote! { lazy::build_query_path(&[#const_name]) },
        FieldIndex::Path(_) => quote! { lazy::build_query_path(#const_name) },
    }
}

fn emit_lazy_field_accessor(
    field: &LedgerField,
    const_name: &Ident,
    field_index: &FieldIndex,
) -> Option<TokenStream> {
    let method_name = make_ident(&field.name);
    let doc = format!(
        "Query the `{}` ledger field ({}) from the node.",
        field.name, field.storage
    );
    let path_expr = query_path_expr(const_name, field_index);

    match field.storage {
        StorageKind::Cell => Some(emit_lazy_cell_accessor(
            &method_name,
            &doc,
            &path_expr,
            field.element_type.as_ref(),
        )),
        StorageKind::Counter => Some(emit_lazy_counter_accessor(&method_name, &doc, &path_expr)),
        StorageKind::Map => Some(emit_lazy_map_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        StorageKind::Set => Some(emit_lazy_set_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        StorageKind::List => Some(emit_lazy_list_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        // Merkle trees don't support single-value lookup via the RPC.
        StorageKind::MerkleTree | StorageKind::HistoricMerkleTree => None,
    }
}

fn emit_lazy_cell_accessor(
    method_name: &Ident,
    doc: &str,
    path_expr: &TokenStream,
    cell_type: Option<&TypeNode>,
) -> TokenStream {
    if let Some(ty) = cell_type {
        let ret_type = lazy_cell_return_type(ty);
        let query_body = lazy_query_body(path_expr);
        quote! {
            #[doc = #doc]
            pub async fn #method_name(&self) -> Result<#ret_type, lazy::ContractError> {
                #query_body
                let av = cell_value(&sv)?;
                Ok(<#ret_type>::try_from(&*av.value).map_err(StateError::Conversion)?)
            }
        }
    } else {
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

fn emit_lazy_counter_accessor(
    method_name: &Ident,
    doc: &str,
    path_expr: &TokenStream,
) -> TokenStream {
    let query_body = lazy_query_body(path_expr);
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self) -> Result<u64, lazy::ContractError> {
            #query_body
            let av = cell_value(&sv)?;
            Ok(<u64>::try_from(&*av.value).map_err(StateError::Conversion)?)
        }
    }
}

fn emit_lazy_map_accessor(
    method_name: &Ident,
    _doc: &str,
    path_expr: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let val_ty = field
        .value
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    let doc = format!("Look up a value by key in the `{}` map (map).", field.name);
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, key: impl Into<AlignedValue>) -> Result<Option<#val_ty>, lazy::ContractError> {
            let mut path = #path_expr;
            path.push(lazy::value_to_query_key(&key.into()));
            let results = self.provider.query_contract_state(
                &self.address,
                vec![lazy::StateQuery { path }],
                self.at_block_hash,
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            let result = results.first().ok_or(lazy::ContractError::NoValue)?;
            // No value and no error means key not found
            if result.value.is_none() && result.error.is_none() {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(result)?;
            let av = cell_value(&sv)?;
            Ok(Some(<#val_ty>::try_from(&*av.value).map_err(StateError::Conversion)?))
        }
    }
}

fn emit_lazy_set_accessor(
    method_name: &Ident,
    _doc: &str,
    path_expr: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let doc = format!("Check if a key exists in the `{}` set (set).", field.name);
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, key: impl Into<AlignedValue>) -> Result<bool, lazy::ContractError> {
            let mut path = #path_expr;
            path.push(lazy::value_to_query_key(&key.into()));
            let results = self.provider.query_contract_state(
                &self.address,
                vec![lazy::StateQuery { path }],
                self.at_block_hash,
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            let result = results.first().ok_or(lazy::ContractError::NoValue)?;
            // Sets store Null for present keys; absent keys have no value
            if result.value.is_none() && result.error.is_none() {
                return Ok(false);
            }
            let _sv = lazy::decode_state_value(result)?;
            Ok(true)
        }
    }
}

fn emit_lazy_list_accessor(
    method_name: &Ident,
    _doc: &str,
    path_expr: &TokenStream,
    field: &LedgerField,
) -> TokenStream {
    let elem_ty = field
        .element_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    let doc = format!(
        "Get an element by index from the `{}` list (list).",
        field.name
    );
    quote! {
        #[doc = #doc]
        pub async fn #method_name(&self, index: usize) -> Result<Option<#elem_ty>, lazy::ContractError> {
            let mut path = #path_expr;
            path.push(lazy::index_to_query_key(index));
            let results = self.provider.query_contract_state(
                &self.address,
                vec![lazy::StateQuery { path }],
                self.at_block_hash,
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            let result = results.first().ok_or(lazy::ContractError::NoValue)?;
            if result.value.is_none() && result.error.is_none() {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(result)?;
            let av = cell_value(&sv)?;
            Ok(Some(<#elem_ty>::try_from(&*av.value).map_err(StateError::Conversion)?))
        }
    }
}

/// The common query + decode preamble shared by all lazy accessors.
///
/// Emits code that:
/// 1. Builds the query path
/// 2. Calls `provider.query_contract_state`
/// 3. Decodes the first result into a `StateValue`
fn lazy_query_body(path_expr: &TokenStream) -> TokenStream {
    quote! {
        let path = #path_expr;
        let results = self.provider.query_contract_state(
            &self.address,
            vec![lazy::StateQuery { path }],
            self.at_block_hash,
        ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
        // One query was sent; a missing result is a malformed RPC response,
        // not a panic.
        let result = results.first().ok_or(lazy::ContractError::NoValue)?;
        let sv = lazy::decode_state_value(result)?;
    }
}

/// Resolve the return type for a lazy cell accessor, unwrapping aliases.
fn lazy_cell_return_type(ty: &TypeNode) -> TokenStream {
    if let TypeNode::Alias { inner, .. } = ty {
        lazy_cell_return_type(inner)
    } else {
        type_to_tokens(ty)
    }
}

// ---------------------------------------------------------------------------
// Circuits struct — async on-chain call methods
// ---------------------------------------------------------------------------

fn emit_circuits_struct(info: &crate::types::ContractInfo, ledger_name: &Ident) -> TokenStream {
    let mut methods = Vec::new();

    for circuit in &info.circuits {
        if circuit.pure || circuit.ir.is_none() {
            continue;
        }

        let sanitized = circuit.name.replace(['$', '-'], "_");
        let method_name = format_ident!("{}", sanitized);
        let circuit_name_str = &circuit.name;
        let ir_const = format_ident!("__IR_{}", sanitized.to_uppercase());

        let doc = format!(
            "Call the `{}` circuit on-chain.\n\n\
             Executes locally, builds a funded transaction, and submits it to the node.",
            circuit.name
        );

        // Declared types of each argument, in the IR vocabulary. The
        // interpreter needs these to slice a struct argument passed as a
        // pre-encoded `AlignedValue` when the circuit destructures it with
        // `Expr::Field`. Serialized here and parsed at runtime, mirroring the
        // embedded helpers/structs/enums constants.
        let arg_types = crate::arg_types::circuit_arg_types(&circuit.arguments);
        let arg_types_json = serde_json::to_string(&arg_types)
            .expect("arg-type serialization cannot fail for valid TypeRefs");

        // The declared result type, embedded the same way: the interpreter
        // encodes the circuit's implicit communication output with it so the
        // output binding matches the canonical runtime's result descriptor.
        let result_type_ref = crate::arg_types::type_node_to_type_ref(&circuit.result_type);
        let result_type_json = serde_json::to_string(&result_type_ref)
            .expect("result-type serialization cannot fail for valid TypeRefs");

        let is_void = super::circuit_calls::is_void_type(&circuit.result_type);

        // Build the return type and tail expression based on void vs non-void
        let (ret_type, tail_expr) = if is_void {
            (
                quote! { Result<(), midnight_contract::ContractError> },
                quote! {
                    let __defs = midnight_contract::CircuitDefs {
                        arg_types: &__arg_types,
                        helpers: &helpers,
                        structs: &structs,
                        enums: &enums,
                        result_type: Some(&__result_type),
                    };
                    let _ = self.contract.call_with(&ir, #circuit_name_str, &__args, &self.witnesses, __defs, &self.coin_encryption_keys, ::core::mem::take(&mut self.shielded)).await?;
                    Ok(())
                },
            )
        } else {
            let result_rust_ty = type_to_tokens(&circuit.result_type);
            let conversion = super::circuit_calls::value_to_type_conversion(
                &circuit.result_type,
                &format!("circuit `{}` return value", circuit.name),
            );
            (
                quote! { Result<#result_rust_ty, midnight_contract::ContractError> },
                quote! {
                    let __defs = midnight_contract::CircuitDefs {
                        arg_types: &__arg_types,
                        helpers: &helpers,
                        structs: &structs,
                        enums: &enums,
                        result_type: Some(&__result_type),
                    };
                    let __result = self.contract.call_with(&ir, #circuit_name_str, &__args, &self.witnesses, __defs, &self.coin_encryption_keys, ::core::mem::take(&mut self.shielded)).await?;
                    let __val = __result.ok_or_else(|| {
                        midnight_contract::runtime::InterpreterError::TypeError(
                            ::std::format!(
                                "circuit `{}` returned no value but its signature is non-void",
                                #circuit_name_str
                            )
                        )
                    })?;
                    // The conversion evaluates to Result<_, InterpreterError>,
                    // which `From`-converts into ContractError.
                    Ok((#conversion)?)
                },
            )
        };

        // Build params and arg bindings
        let (params, args_expr) = if circuit.arguments.is_empty() {
            (
                quote! {},
                quote! { let __args: [(&str, midnight_contract::runtime::Value); 0] = []; },
            )
        } else {
            let param_list: Vec<_> = circuit
                .arguments
                .iter()
                .map(|arg| {
                    let name = make_ident(&arg.name);
                    if super::circuit_calls::has_typed_conversion(&arg.type_node) {
                        let ty = type_to_tokens(&arg.type_node);
                        quote! { #name: #ty }
                    } else {
                        quote! { #name: midnight_contract::runtime::Value }
                    }
                })
                .collect();

            let binding_list: Vec<_> = circuit
                .arguments
                .iter()
                .map(|arg| {
                    let name_str = &arg.name;
                    let name_ident = make_ident(&arg.name);
                    let conversion =
                        super::circuit_calls::type_to_value_conversion(&name_ident, &arg.type_node);
                    quote! { (#name_str, #conversion) }
                })
                .collect();

            (
                quote! { , #(#param_list),* },
                quote! { let __args = [#(#binding_list),*]; },
            )
        };

        // The embedded constants are validated (serialized + re-parsed) at
        // codegen time by `validate::check_embedded_json`, so these runtime
        // parses are belt and braces: they only fail if the compiled-in
        // string was somehow corrupted, and then they surface as an error
        // instead of a panic.
        methods.push(quote! {
            #[doc = #doc]
            pub async fn #method_name(&mut self #params) -> #ret_type {
                let ir: midnight_contract::compact_codegen::ir::CircuitIrBody =
                    serde_json::from_str(#ledger_name::#ir_const).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded IR for circuit `{}` is invalid JSON: {}",
                            #circuit_name_str,
                            __e
                        ))
                    })?;
                let helpers: Vec<midnight_contract::compact_codegen::ir::HelperDef> =
                    serde_json::from_str(#ledger_name::__HELPERS_JSON).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded helper definitions are invalid JSON: {}", __e
                        ))
                    })?;
                let structs: Vec<midnight_contract::compact_codegen::ir::StructDef> =
                    serde_json::from_str(#ledger_name::__STRUCTS_JSON).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded struct definitions are invalid JSON: {}", __e
                        ))
                    })?;
                let enums: Vec<midnight_contract::compact_codegen::ir::EnumDef> =
                    serde_json::from_str(#ledger_name::__ENUMS_JSON).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded enum definitions are invalid JSON: {}", __e
                        ))
                    })?;
                #args_expr
                // Argument types, parsed once and borrowed for the call. The
                // owned vec must outlive `__arg_types`, which holds `&str`
                // views into its names.
                let __arg_types_owned: Vec<(String, midnight_contract::compact_codegen::ir::TypeRef)> =
                    serde_json::from_str(#arg_types_json).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded argument types for circuit `{}` are invalid JSON: {}",
                            #circuit_name_str,
                            __e
                        ))
                    })?;
                let __arg_types: Vec<(&str, midnight_contract::compact_codegen::ir::TypeRef)> =
                    __arg_types_owned
                        .iter()
                        .map(|(__n, __t)| (__n.as_str(), __t.clone()))
                        .collect();
                let __result_type: midnight_contract::compact_codegen::ir::TypeRef =
                    serde_json::from_str(#result_type_json).map_err(|__e| {
                        midnight_contract::ContractError::Serialization(::std::format!(
                            "embedded result type for circuit `{}` is invalid JSON: {}",
                            #circuit_name_str,
                            __e
                        ))
                    })?;
                #tail_expr
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
        /// For circuits that call witnesses, chain [`Circuits::with_witnesses`]
        /// with a typed [`Witnesses`] impl. When a `PrivateStateProvider` is
        /// attached, the contract's private state is threaded automatically,
        /// keyed by the contract address.
        pub struct Circuits<'a, P, Wp = midnight_contract::runtime::NoWitnesses> {
            contract: &'a midnight_contract::Contract<P>,
            witnesses: Wp,
            coin_encryption_keys: Vec<(
                midnight_contract::CoinPublicKey,
                midnight_contract::EncryptionPublicKey,
            )>,
            shielded: midnight_contract::ShieldedInputs,
        }

        #with_witnesses_impl

        impl<'a, P, Wp> Circuits<'a, P, Wp> {
            /// Attach wallet coins to spend as shielded (Zswap) inputs for the
            /// next circuit call, funding a circuit that receives one of the
            /// caller's own coins (e.g. `receiveShielded`).
            ///
            /// Each coin is selected exactly by its nullifier — so the coin the
            /// circuit re-commits (`nonce`/`color`/`value`) is the one spent —
            /// and routed to the segment of the output it funds. Enumerate the
            /// wallet's coins with `MidnightProvider::spendable_shielded_coins`,
            /// build the circuit's `ShieldedCoinInfo` argument from the same
            /// coin's `nonce`/`token_type`/`value`, and pass the coin here.
            ///
            /// The attached inputs apply to the next call on this builder.
            pub fn with_shielded_inputs(
                mut self,
                coins: impl IntoIterator<Item = midnight_contract::SpendableShieldedCoin>,
            ) -> Self {
                self.shielded.coins = coins.into_iter().collect();
                self
            }

            /// Attach `coin_public_key -> encryption_public_key` mappings for the
            /// shielded coins these circuit calls create (e.g. via
            /// `mintShieldedToken`). The SDK adds a discovery ciphertext to each
            /// matching output so the recipient's wallet finds the coin through
            /// normal sync, no `watchFor`. Without a mapping, an external
            /// recipient would have to scan for the coin explicitly.
            pub fn with_coin_encryption_keys(
                mut self,
                keys: impl IntoIterator<
                    Item = (
                        midnight_contract::CoinPublicKey,
                        midnight_contract::EncryptionPublicKey,
                    ),
                >,
            ) -> Self {
                self.coin_encryption_keys = keys.into_iter().collect();
                self
            }
        }

        impl<'a, P, Wp> Circuits<'a, P, Wp>
        where
            P: midnight_contract::AsMidnightProvider,
            P: midnight_contract::Provider,
            Wp: midnight_contract::runtime::WitnessProvider,
        {
            #(#methods)*
        }
    }
}

fn cell_accessor(ty: &TypeNode, nav: &TokenStream) -> (TokenStream, TokenStream) {
    if let TypeNode::Alias { inner, .. } = ty {
        cell_accessor(inner, nav)
    } else {
        let ret_type = type_to_tokens(ty);
        let body = cell_value_body(&ret_type, nav);
        (ret_type, body)
    }
}

/// Generate the body for a cell accessor that uses `cell_value` + `TryFrom<&ValueSlice>`.
fn cell_value_body(ret_type: &TokenStream, nav: &TokenStream) -> TokenStream {
    quote! {
        let sv = #nav?;
        let av = cell_value(sv)?;
        <#ret_type>::try_from(&*av.value).map_err(StateError::Conversion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_cell_defaults_to_zero_not_unit() {
        let field = LedgerField {
            name: "nonce".to_string(),
            index: serde_json::json!(0),
            storage: StorageKind::Cell,
            exported: true,
            element_type: Some(TypeNode::Bytes { length: 32 }),
            key: None,
            value: None,
            depth: None,
        };
        let out = emit_initial_state(&[field], "Gateway")
            .to_string()
            .replace(' ', "");
        assert!(
            out.contains("nonce:AlignedValue::from(Bytes([0u8;32]))"),
            "Bytes<N> cell must default to a zero-filled value, got: {out}"
        );
        assert!(!out.contains("nonce:AlignedValue::from(())"));
    }
}
