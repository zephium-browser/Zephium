//! Optional grants and complete revocation effects at the durable browser edge.

use serde_json::{json, Value};

use crate::browser::{BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::permissions::{InstallPermissions, PermissionRequest, RevocationEffect};
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "permissions.contains",
    "permissions.getAll",
    "permissions.request",
    "permissions.remove",
    "permissions.addHostAccessRequest",
    "permissions.removeHostAccessRequest",
];

/// Permission state changes are committed by the adapter, never optimistically.
#[derive(Clone, Debug)]
pub enum Request {
    /// Prompt during this authenticated gesture, commit accepted grants, then emit onAdded.
    Prompt {
        /// Requested API names and host patterns.
        request: PermissionRequest,
    },
    /// Recompute against current serialized grants, persist, and apply every effect before success.
    Remove {
        /// Exact optional permissions to revoke.
        request: PermissionRequest,
        /// Proposed post-commit state; never overwrite concurrent grant changes with this preview.
        updated: Box<InstallPermissions>,
        /// Preview of required teardown and onRemoved payload; recompute at commit.
        effect: RevocationEffect,
    },
}

impl Request {
    /// Rebase a queued removal on the current install grants before durable commit.
    pub fn rebase_removal(
        &self,
        current: &InstallPermissions,
    ) -> Result<Option<(InstallPermissions, RevocationEffect)>, ApiError> {
        let Self::Remove { request, .. } = self else {
            return Ok(None);
        };
        let mut updated = current.clone();
        let effect = updated.remove(request)?;
        Ok(Some((updated, effect)))
    }
}

/// Chrome-shaped payload for getAll and permission change events.
pub fn to_value(request: &PermissionRequest) -> Value {
    json!({"permissions": request.permissions.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "origins": request.origins.iter().map(|p| p.as_str()).collect::<Vec<_>>()})
}

fn parse(value: &Value) -> Result<PermissionRequest, ApiError> {
    let strings = |key| {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    PermissionRequest::parse(&strings("permissions"), &strings("origins"))
}

fn handle(host: &mut impl BrowserPort, call: Invocation<'_>) -> Handled {
    if call.name() == "permissions.getAll" {
        let state = call.permissions().snapshot();
        return call.reply(Ok(
            json!({"permissions":state.permissions,"origins":state.origins}),
        ));
    }
    if matches!(
        call.name(),
        "permissions.addHostAccessRequest" | "permissions.removeHostAccessRequest"
    ) {
        return call.reply(Err(crate::browser::unsupported()));
    }
    let request = match parse(&call.args()[0]) {
        Ok(request) => request,
        Err(error) => return call.reply(Err(error)),
    };
    match call.name() {
        "permissions.contains" => {
            let contains = call.permissions().contains(&request);
            call.reply(Ok(json!(contains)))
        }
        "permissions.request" => {
            if !host.user_gesture(call.sender()) {
                return call.reply(Err(ApiError::other(
                    "This function must be called during a user gesture",
                )));
            }
            if let Err(error) = call.permissions().check_requestable(&request) {
                return call.reply(Err(error));
            }
            if call.permissions().contains(&request) {
                return call.reply(Ok(json!(true)));
            }
            host.request(
                BrowserRequest::Permissions(Request::Prompt { request }),
                call,
            )
        }
        "permissions.remove" => {
            let mut updated = call.permissions().clone();
            let effect = match updated.remove(&request) {
                Ok(effect) => effect,
                Err(error) => return call.reply(Err(error)),
            };
            host.request(
                BrowserRequest::Permissions(Request::Remove {
                    request,
                    updated: Box::new(updated),
                    effect,
                }),
                call,
            )
        }
        _ => call.reply(Err(crate::browser::unsupported())),
    }
}

/// Registers generated permission and argument policy.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in METHODS {
        table.register_generated(name, |host: &mut S, call| handle(host, call))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
