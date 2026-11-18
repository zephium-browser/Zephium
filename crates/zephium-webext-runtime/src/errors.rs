//! The one error type an extension API call settles with.
//!
//! Extensions see only `message`, so every generic failure uses Chrome's exact
//! text. [`ApiErrorKind`] exists for the host: tests, counters and teardown
//! decisions, never for the wire.

use std::borrow::Cow;
use std::fmt;

/// Why a call failed, as the host classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApiErrorKind {
    /// No such `namespace.method` is registered.
    UnknownMethod,
    /// The method exists but the calling context may not use it.
    NotAvailableInContext,
    /// The install lacks the API permission the method requires.
    MissingPermission,
    /// The arguments do not decode or do not fit the method's signature.
    InvalidArguments,
    /// The install may not touch the target tab or frame.
    TargetDenied,
    /// A message or reply exceeded the context's byte limit.
    MessageTooLarge,
    /// The context's call rate, concurrency or byte budget is exhausted.
    Overloaded,
    /// The calling context or its runtime generation is gone.
    ContextInvalidated,
    /// A `permissions` API request was refused.
    PermissionRequest,
    /// Any method-specific failure.
    Other,
}

/// A settled API failure with the text the extension observes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    kind: ApiErrorKind,
    message: Cow<'static, str>,
}

const ACCESS_DENIED: &str = "Access to extension API denied.";
const MESSAGE_TOO_LARGE: &str = "Message exceeded maximum allowed size of 64MiB.";
const CONTEXT_INVALIDATED: &str = "Extension context invalidated.";
const CANNOT_ACCESS_PAGE: &str = "Cannot access contents of the page. Extension manifest must request permission to access the respective host.";
const CANNOT_SCRIPT_GALLERY: &str = "The extensions gallery cannot be scripted.";
const CANNOT_ACCESS_OTHER_EXTENSION: &str =
    "Cannot access a chrome-extension:// URL of different extension";
const NOT_IN_MANIFEST: &str = "Only permissions specified in the manifest may be requested.";
const CANNOT_REMOVE_REQUIRED: &str = "You cannot remove required permissions.";
const NO_MATCHING_SIGNATURE: &str = "No matching signature.";
// Chrome has no per-context rate or concurrency limit; these two are ours.
const RATE_LIMITED: &str = "Too many extension API calls in a short time.";
const TOO_MANY_PENDING: &str = "Too many extension API calls are pending.";

impl ApiError {
    const fn fixed(kind: ApiErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message: Cow::Borrowed(message),
        }
    }

    /// Chrome's host-side answer for a function it does not know.
    pub const fn unknown_method() -> Self {
        Self::fixed(ApiErrorKind::UnknownMethod, ACCESS_DENIED)
    }

    /// Chrome's host-side answer for a function the extension lacks the
    /// permission for. Chrome uses the same text as for an unknown function.
    pub const fn missing_permission() -> Self {
        Self::fixed(ApiErrorKind::MissingPermission, ACCESS_DENIED)
    }

    /// Chrome's binding error for an API the calling context cannot use.
    pub fn not_available_in_context(name: &str) -> Self {
        Self {
            kind: ApiErrorKind::NotAvailableInContext,
            message: Cow::Owned(format!("'{name}' is not available in this context.")),
        }
    }

    /// Chrome's invocation error. `signature` is the method's parameter list
    /// as Chrome prints it, for example `integer tabId, function callback`.
    pub fn invalid_arguments(name: &str, signature: &str, detail: &str) -> Self {
        Self {
            kind: ApiErrorKind::InvalidArguments,
            message: Cow::Owned(format!(
                "Error in invocation of {name}({signature}): {detail}"
            )),
        }
    }

    /// [`Self::invalid_arguments`] with Chrome's generic detail.
    pub fn no_matching_signature(name: &str, signature: &str) -> Self {
        Self::invalid_arguments(name, signature, NO_MATCHING_SIGNATURE)
    }

    /// ChromeSetting's refusal when a setting-specific value does not validate.
    pub const fn invalid_invocation() -> Self {
        Self::fixed(ApiErrorKind::InvalidArguments, "Invalid invocation")
    }

    /// Zephium's malformed transport refusal, using Chrome's denial text.
    pub const fn malformed_arguments() -> Self {
        Self::fixed(ApiErrorKind::InvalidArguments, ACCESS_DENIED)
    }

    /// Chrome's messaging error text. Zephium's transport limits are lower
    /// than the Chromium ceiling named in this compatibility message.
    pub const fn message_too_large() -> Self {
        Self::fixed(ApiErrorKind::MessageTooLarge, MESSAGE_TOO_LARGE)
    }

    /// Chrome's error for a context whose extension was reloaded or removed.
    pub const fn context_invalidated() -> Self {
        Self::fixed(ApiErrorKind::ContextInvalidated, CONTEXT_INVALIDATED)
    }

    /// Chrome's host-permission refusal. Chrome's longer variant quotes the
    /// URL; this one never reveals a URL the extension may not read.
    pub const fn cannot_access_page() -> Self {
        Self::fixed(ApiErrorKind::TargetDenied, CANNOT_ACCESS_PAGE)
    }

    /// Chrome's refusal to script the Chrome Web Store.
    pub const fn cannot_script_gallery() -> Self {
        Self::fixed(ApiErrorKind::TargetDenied, CANNOT_SCRIPT_GALLERY)
    }

    /// Chrome's refusal to script another extension's pages.
    pub const fn cannot_access_other_extension() -> Self {
        Self::fixed(ApiErrorKind::TargetDenied, CANNOT_ACCESS_OTHER_EXTENSION)
    }

    /// `permissions.request` named something the manifest did not declare.
    pub const fn not_in_manifest() -> Self {
        Self::fixed(ApiErrorKind::PermissionRequest, NOT_IN_MANIFEST)
    }

    /// `permissions.remove` named a required permission.
    pub const fn cannot_remove_required() -> Self {
        Self::fixed(ApiErrorKind::PermissionRequest, CANNOT_REMOVE_REQUIRED)
    }

    /// Chrome's text for a permission name it does not recognise.
    pub fn unknown_permission(name: &str) -> Self {
        Self {
            kind: ApiErrorKind::PermissionRequest,
            message: Cow::Owned(format!("'{name}' is not a recognized permission.")),
        }
    }

    /// Chrome's text for an origin pattern that does not parse.
    pub fn invalid_origin(pattern: &str, reason: &str) -> Self {
        Self {
            kind: ApiErrorKind::PermissionRequest,
            message: Cow::Owned(format!(
                "Invalid value for origin pattern {pattern}: {reason}"
            )),
        }
    }

    /// The context exceeded its token bucket. Zephium-specific.
    pub const fn rate_limited() -> Self {
        Self::fixed(ApiErrorKind::Overloaded, RATE_LIMITED)
    }

    /// The context or install has too many calls or bytes in flight.
    /// Zephium-specific.
    pub const fn too_many_pending() -> Self {
        Self::fixed(ApiErrorKind::Overloaded, TOO_MANY_PENDING)
    }

    /// A method-specific failure with its Chrome text.
    pub fn other(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind: ApiErrorKind::Other,
            message: message.into(),
        }
    }

    /// The host-side classification.
    pub const fn kind(&self) -> ApiErrorKind {
        self.kind
    }

    /// The exact text the extension observes in `lastError` or a rejection.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_failures_use_chromes_text() {
        assert_eq!(
            ApiError::unknown_method().message(),
            "Access to extension API denied."
        );
        assert_eq!(
            ApiError::missing_permission().message(),
            ApiError::unknown_method().message()
        );
        assert_eq!(
            ApiError::not_available_in_context("runtime.connectNative").message(),
            "'runtime.connectNative' is not available in this context."
        );
        assert_eq!(
            ApiError::no_matching_signature("tabs.get", "integer tabId, function callback")
                .message(),
            "Error in invocation of tabs.get(integer tabId, function callback): No matching signature."
        );
        assert_eq!(
            ApiError::message_too_large().message(),
            "Message exceeded maximum allowed size of 64MiB."
        );
    }

    #[test]
    fn kinds_distinguish_failures_that_share_text() {
        assert_ne!(
            ApiError::unknown_method().kind(),
            ApiError::missing_permission().kind()
        );
        assert_eq!(ApiError::rate_limited().kind(), ApiErrorKind::Overloaded);
    }
}
