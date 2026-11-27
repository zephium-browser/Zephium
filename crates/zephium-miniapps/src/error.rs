use thiserror::Error;

/// Every refusal is explicit: installing a mini app is a trust decision, and a
/// caller must never receive a half-valid descriptor.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum MiniAppError {
    #[error("the web app manifest exceeds the accepted size")]
    ManifestTooLarge,
    #[error("the web app manifest is not valid JSON")]
    InvalidJson,
    #[error("the web app manifest has no usable start_url")]
    MissingStartUrl,
    #[error("the address is not an absolute HTTPS URL")]
    InsecureUrl,
    #[error("the address contains embedded credentials")]
    CredentialsInUrl,
    #[error("the scope lies outside the start address origin")]
    ScopeOutsideOrigin,
    #[error("the start address lies outside the declared scope")]
    StartOutsideScope,
    #[error("the app name is empty or unreasonable")]
    InvalidName,
    #[error("the app registry is full")]
    RegistryFull,
    #[error("the app is not installed")]
    NotFound,
    #[error("the stored app registry is not valid")]
    InvalidRegistry,
    #[error("the app registry exceeds the accepted size")]
    RegistryTooLarge,
}
