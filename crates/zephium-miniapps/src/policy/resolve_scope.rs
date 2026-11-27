use crate::policy::normalize_https_url::normalize_https_url;
use crate::{error::MiniAppError, policy::same_origin::same_origin};
use url::Url;

/// Returns a path-scoped URL without a query or fragment. An absent scope uses
/// the start address's directory, which is what the Web App Manifest default
/// means in practice.
pub fn resolve_scope(
    manifest_base: &Url,
    start_url: &Url,
    raw_scope: Option<&str>,
) -> Result<Url, MiniAppError> {
    let mut scope = match raw_scope.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => normalize_https_url(manifest_base, raw)?,
        None => {
            let mut origin = start_url.clone();
            origin.set_query(None);
            origin.set_fragment(None);
            let path = start_url.path();
            let directory = path.rfind('/').map_or("/", |at| &path[..=at]).to_owned();
            origin.set_path(if directory.is_empty() {
                "/"
            } else {
                &directory
            });
            origin
        }
    };
    if !same_origin(&scope, start_url) {
        return Err(MiniAppError::ScopeOutsideOrigin);
    }
    if !scope.path().starts_with('/') {
        return Err(MiniAppError::ScopeOutsideOrigin);
    }
    scope.set_query(None);
    scope.set_fragment(None);
    Ok(scope)
}
