use crate::error::MiniAppError;
use url::Url;

/// Joins a manifest-relative address and admits only public HTTPS navigation.
/// Credentials, opaque schemes and fragments are refused before any component
/// downstream can treat the result as trusted.
pub fn normalize_https_url(base: &Url, raw: &str) -> Result<Url, MiniAppError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2_048 {
        return Err(MiniAppError::InsecureUrl);
    }
    let mut url = base.join(raw).map_err(|_| MiniAppError::InsecureUrl)?;
    if url.scheme() != "https" {
        return Err(MiniAppError::InsecureUrl);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(MiniAppError::InsecureUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(MiniAppError::CredentialsInUrl);
    }
    url.set_fragment(None);
    Ok(url)
}
