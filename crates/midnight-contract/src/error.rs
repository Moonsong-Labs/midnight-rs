use midnight_provider::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("contract not found at address {0}")]
    NotFound(String),

    #[error("state deserialization error: {0}")]
    State(#[from] midnight_bindgen::StateError),
}
