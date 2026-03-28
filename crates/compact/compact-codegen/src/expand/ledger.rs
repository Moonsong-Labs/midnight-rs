use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::types::{FieldIndex, LedgerField, TypeNode};

use super::helpers::make_ident;
use super::types::type_to_tokens;

pub(crate) fn emit_ledger_wrapper(
    fields: &[LedgerField],
    name: &str,
    circuit_call_methods: &TokenStream,
    info: &crate::types::ContractInfo,
) -> TokenStream {
    let struct_name = format_ident!("{}", name);

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
                let state = midnight_contract::fetch_state(provider, address).await?;
                Ok(Self::new(state))
            }

            #state_accessor

            #(#accessors)*

            #circuit_call_methods
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
        /// let mut contract = Contract::deploy()
        ///     .provider(&provider)
        ///     .initial_state(LedgerInitialState { round: 0 })
        ///     .zk_keys("compiled")
        ///     .deploy()
        ///     .await?;
        ///
        /// contract.circuits().increment().await?;
        /// let ledger = contract.ledger();
        /// ```
        pub struct Contract<P>(midnight_contract::Contract<P>);

        impl Contract<()> {
            /// Start building a new contract deployment.
            pub fn deploy() -> ContractDeployBuilder {
                ContractDeployBuilder(midnight_contract::ContractBuilder::new())
            }
        }

        /// Builder wrapper that returns the generated `Contract<P>` on deploy.
        pub struct ContractDeployBuilder(midnight_contract::ContractBuilder);

        impl ContractDeployBuilder {
            pub fn provider<Q>(self, provider: Q) -> ContractDeployBuilderWithProvider<Q> {
                ContractDeployBuilderWithProvider(self.0.provider(provider))
            }
        }

        pub struct ContractDeployBuilderWithProvider<P>(midnight_contract::ContractBuilder<P>);

        impl<P> ContractDeployBuilderWithProvider<P> {
            pub fn initial_state(self, state: impl Into<ContractState<InMemoryDB>>) -> Self {
                Self(self.0.initial_state(state))
            }

            pub fn zk_keys(self, path: impl Into<std::path::PathBuf>) -> Self {
                Self(self.0.zk_keys(path))
            }
        }

        impl ContractDeployBuilderWithProvider<midnight_provider::MidnightProvider> {
            pub async fn deploy(self) -> Result<Contract<midnight_provider::MidnightProvider>, midnight_contract::ContractError> {
                Ok(Contract(self.0.deploy().await?))
            }
        }

        impl<'a> ContractDeployBuilderWithProvider<&'a midnight_provider::MidnightProvider> {
            pub async fn deploy(self) -> Result<Contract<&'a midnight_provider::MidnightProvider>, midnight_contract::ContractError> {
                Ok(Contract(self.0.deploy().await?))
            }
        }

        impl<P: midnight_contract::Provider> Contract<P> {
            /// The contract's on-chain address (hex string).
            pub fn address(&self) -> &str {
                self.0.address()
            }

            /// The current cached contract state.
            pub fn state(&self) -> &ContractState<InMemoryDB> {
                self.0.state()
            }

            /// Get the typed ledger from the cached state.
            pub fn ledger(&self) -> #struct_name {
                #struct_name::new(self.0.state().clone())
            }

            /// Refresh the cached state from the chain.
            pub async fn sync(&mut self) -> Result<(), midnight_contract::ContractError> {
                self.0.sync().await
            }

            /// Access on-chain circuit call methods.
            pub fn circuits(&mut self) -> Circuits<'_, P> {
                Circuits(&mut self.0)
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

    match field.storage_kind().as_str() {
        "cell" => emit_cell_accessor(&method_name, &doc, &nav, field.cell_type.as_ref()),
        "counter" => emit_counter_accessor(&method_name, &doc, &nav),
        "map" => emit_map_accessor(&method_name, &doc, &nav, field),
        "set" => emit_set_accessor(&method_name, &doc, &nav, field),
        "list" => emit_list_accessor(&method_name, &doc, &nav, field),
        "merkle-tree" | "historic-merkle-tree" => {
            emit_merkle_tree_accessor(&method_name, &doc, &nav)
        }
        _ => {
            quote! {
                #[doc = #doc]
                pub fn #method_name(&self) -> Result<&StateValue<InMemoryDB>, StateError> {
                    #nav
                }
            }
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
        .key_type
        .as_ref()
        .map_or_else(|| quote! { Vec<u8> }, type_to_tokens);
    let val_ty = field
        .value_type
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
                    expected: "Set",
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

        match field.storage_kind().as_str() {
            "cell" => {
                // Use typed fields only for simple scalar types that have
                // Default + Into<AlignedValue>. Complex types use AlignedValue.
                let is_simple = matches!(
                    &field.cell_type,
                    Some(TypeNode::Uint { .. }) | Some(TypeNode::Boolean)
                );
                if is_simple {
                    let rust_type = type_to_tokens(field.cell_type.as_ref().unwrap());
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: #rust_type });
                    field_defaults.push(quote! { #field_name: Default::default() });
                    field_conversions
                        .push(quote! { StateValue::from(AlignedValue::from(self.#field_name)) });
                } else {
                    field_defs.push(quote! { #[doc = #doc] pub #field_name: AlignedValue });
                    field_defaults.push(quote! { #field_name: AlignedValue::from(()) });
                    field_conversions.push(quote! { StateValue::from(self.#field_name.clone()) });
                }
            }
            "counter" => {
                field_defs.push(quote! { #[doc = #doc] pub #field_name: u64 });
                field_defaults.push(quote! { #field_name: 0 });
                field_conversions.push(quote! { StateValue::from(self.#field_name) });
            }
            "map" | "set" => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StorageHashMap<AlignedValue, StateValue<InMemoryDB>>
                });
                field_defaults.push(quote! { #field_name: StorageHashMap::new() });
                field_conversions.push(quote! { StateValue::Map(self.#field_name) });
            }
            "list" => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StateValue::Array(StorageArray::new()) });
                field_conversions.push(quote! { self.#field_name });
            }
            "merkle-tree" | "historic-merkle-tree" => {
                field_defs.push(quote! {
                    #[doc = #doc]
                    pub #field_name: StateValue<InMemoryDB>
                });
                field_defaults.push(quote! { #field_name: StateValue::Null });
                field_conversions.push(quote! { self.#field_name });
            }
            _ => {
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
        }

        impl<P: lazy::StateQueryProvider> #struct_name<P> {
            /// Create a new lazy query handle for the given contract address.
            pub fn new(provider: P, address: impl Into<String>) -> Self {
                Self { address: address.into(), provider }
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

    match field.storage_kind().as_str() {
        "cell" => Some(emit_lazy_cell_accessor(
            &method_name,
            &doc,
            &path_expr,
            field.cell_type.as_ref(),
        )),
        "counter" => Some(emit_lazy_counter_accessor(&method_name, &doc, &path_expr)),
        "map" => Some(emit_lazy_map_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        "set" => Some(emit_lazy_set_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        "list" => Some(emit_lazy_list_accessor(
            &method_name,
            &doc,
            &path_expr,
            field,
        )),
        // Merkle trees don't support single-value lookup via the RPC.
        _ => None,
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
        .value_type
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
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            // No value and no error means key not found
            if results[0].value.is_none() && results[0].error.is_none() {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(&results[0])?;
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
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            // Sets store Null for present keys; absent keys have no value
            Ok(results[0].value.is_some())
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
            ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
            if results[0].value.is_none() && results[0].error.is_none() {
                return Ok(None);
            }
            let sv = lazy::decode_state_value(&results[0])?;
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
        ).await.map_err(|e| lazy::ContractError::Provider(Box::new(e)))?;
        let sv = lazy::decode_state_value(&results[0])?;
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

        let sanitized = circuit.name.replace('$', "_").replace('-', "_");
        let method_name = format_ident!("{}", sanitized);
        let circuit_name_str = &circuit.name;
        let ir_const = format_ident!("__IR_{}", sanitized.to_uppercase());

        let doc = format!(
            "Call the `{}` circuit on-chain.\n\n\
             Executes locally, builds a funded transaction, and submits it to the node.",
            circuit.name
        );

        methods.push(quote! {
            #[doc = #doc]
            pub async fn #method_name(&mut self) -> Result<(), midnight_contract::ContractError> {
                let ir: midnight_contract::compact_codegen::ir::CircuitIrBody =
                    serde_json::from_str(#ledger_name::#ir_const).expect(
                        concat!("embedded IR for `", #circuit_name_str, "` must be valid JSON")
                    );
                self.0.call(&ir, #circuit_name_str).await
            }
        });
    }

    quote! {
        /// On-chain circuit call methods.
        ///
        /// Access via `contract.circuits()`. Each method executes the circuit
        /// locally, builds a funded transaction, and submits it to the node.
        pub struct Circuits<'a, P>(&'a mut midnight_contract::Contract<P>);

        impl<'a, P> Circuits<'a, P>
        where
            P: std::ops::Deref<Target = midnight_provider::MidnightProvider>,
            P: midnight_contract::Provider,
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
