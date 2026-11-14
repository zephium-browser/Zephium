use serde_json::Value;

use crate::api::ArgumentValidator;
use crate::context::{ContextKind, ContextMask, Sender};
use crate::dispatch::{ApiTable, Invocation, MethodSpec, RegistrationError};
use crate::errors::ApiError;
use crate::permissions::Gate;

use super::{AccessLevel, Area, GetKeys, Mutation, Storage, StorageChangeEvent};

/// The adapter's per-install storage lookup, clock and committed-event sink.
/// Execute calls on a storage executor when the backend performs blocking I/O.
pub trait StorageHost {
    /// Returns only this authenticated sender's store. Storage also checks
    /// profile/install/incognito identity independently before handling a call.
    fn storage(&mut self, sender: &Sender) -> Result<&mut Storage, ApiError>;
    /// Monotonic time, shared by all contexts of an install.
    fn storage_now_nanos(&self) -> u64;
    /// Routes a committed change to the install's `storage.onChanged` and
    /// area-specific listeners. Filter recipient access again at delivery.
    fn storage_changed(&mut self, sender: &Sender, change: StorageChangeEvent);
    /// Replaceable argument-validation seam for generated schema validators.
    fn validate_storage_arguments(&self, method: &str, args: &[Value]) -> Result<(), ApiError> {
        ManualValidator.validate(method, args)
    }
}

/// Hand-written Chrome signatures, replaceable at the adapter boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManualValidator;

impl ArgumentValidator for ManualValidator {
    fn validate(&self, method: &str, args: &[Value]) -> Result<(), ApiError> {
        let member = method.rsplit('.').next().unwrap_or("");
        let valid = match member {
            "get" => {
                args.is_empty()
                    || (args.len() == 1
                        && (args[0].is_null() || args[0].is_object() || string_keys(&args[0])))
            }
            "getBytesInUse" => {
                args.is_empty() || (args.len() == 1 && (args[0].is_null() || string_keys(&args[0])))
            }
            "set" => args.len() == 1 && args[0].is_object(),
            "remove" => args.len() == 1 && string_keys(&args[0]),
            "clear" | "getKeys" => args.is_empty(),
            "setAccessLevel" => {
                args.len() == 1
                    && args[0].as_object().is_some_and(|options| {
                        options.len() == 1
                            && options.get("accessLevel").is_some_and(|level| {
                                matches!(
                                    level.as_str(),
                                    Some("TRUSTED_CONTEXTS" | "TRUSTED_AND_UNTRUSTED_CONTEXTS")
                                )
                            })
                    })
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(ApiError::no_matching_signature(method, signature(member)))
        }
    }
}

fn signature(member: &str) -> &'static str {
    match member {
        "get" => "optional [string|array|object] keys, optional function callback",
        "getBytesInUse" => "optional [string|array] keys, optional function callback",
        "set" => "object items, optional function callback",
        "remove" => "[string|array] keys, optional function callback",
        "setAccessLevel" => "object accessOptions, optional function callback",
        _ => "optional function callback",
    }
}

fn string_keys(value: &Value) -> bool {
    value.is_string()
        || value
            .as_array()
            .is_some_and(|keys| keys.iter().all(Value::is_string))
}

fn keys(value: &Value) -> Vec<String> {
    match value {
        Value::String(key) => vec![key.clone()],
        Value::Array(values) => values
            .iter()
            .filter_map(|key| key.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn handle<S: StorageHost>(
    host: &mut S,
    invocation: &Invocation<'_>,
    area: Area,
) -> Result<Value, ApiError> {
    host.validate_storage_arguments(invocation.name(), invocation.args())?;
    let now = host.storage_now_nanos();
    let sender = invocation.sender();
    let unlimited = invocation.permissions().has_api("unlimitedStorage");
    let storage = host.storage(sender)?;
    storage.check_access(sender, area)?;
    let args = invocation.args();
    let member = invocation.name().rsplit('.').next().unwrap_or("");
    let mutation = match member {
        "get" => {
            let selector = match args.first() {
                None | Some(Value::Null) => GetKeys::All,
                Some(Value::Object(defaults)) => GetKeys::Defaults(defaults.clone()),
                Some(value) => GetKeys::Keys(keys(value)),
            };
            return storage
                .get(area, &selector)
                .map(Value::Object)
                .map_err(Into::into);
        }
        "getBytesInUse" => {
            let selector = args.first().filter(|value| !value.is_null()).map(keys);
            return storage
                .get_bytes_in_use(area, selector.as_deref())
                .map(Value::from)
                .map_err(Into::into);
        }
        "getKeys" => {
            return storage
                .get_keys(area)
                .map(|keys| Value::Array(keys.into_iter().map(Value::String).collect()))
                .map_err(Into::into)
        }
        "setAccessLevel" => {
            let access = if args
                .first()
                .and_then(|options| options.get("accessLevel"))
                .and_then(Value::as_str)
                == Some("TRUSTED_CONTEXTS")
            {
                AccessLevel::TrustedContexts
            } else {
                AccessLevel::TrustedAndUntrustedContexts
            };
            storage.set_access_level(sender, area, access)?;
            return Ok(Value::Null);
        }
        "set" => Mutation::Set(
            args.first()
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(ApiError::malformed_arguments)?,
        ),
        "remove" => Mutation::Remove(
            args.first()
                .map(keys)
                .ok_or_else(ApiError::malformed_arguments)?,
        ),
        "clear" => Mutation::Clear,
        _ => return Err(ApiError::unknown_method()),
    };
    let change = storage.mutate(area, &mutation, unlimited, now)?;
    if let Some(change) = change {
        host.storage_changed(sender, change);
    }
    Ok(Value::Null)
}

/// Registers all four areas with storage permission gates. Access policies
/// are enforced at call time, including session restrictions and identity.
pub fn register<S: StorageHost + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    const AREAS: [(Area, [&str; 7]); 4] = [
        (
            Area::Local,
            [
                "storage.local.get",
                "storage.local.set",
                "storage.local.remove",
                "storage.local.clear",
                "storage.local.getBytesInUse",
                "storage.local.getKeys",
                "storage.local.setAccessLevel",
            ],
        ),
        (
            Area::Sync,
            [
                "storage.sync.get",
                "storage.sync.set",
                "storage.sync.remove",
                "storage.sync.clear",
                "storage.sync.getBytesInUse",
                "storage.sync.getKeys",
                "storage.sync.setAccessLevel",
            ],
        ),
        (
            Area::Session,
            [
                "storage.session.get",
                "storage.session.set",
                "storage.session.remove",
                "storage.session.clear",
                "storage.session.getBytesInUse",
                "storage.session.getKeys",
                "storage.session.setAccessLevel",
            ],
        ),
        (
            Area::Managed,
            [
                "storage.managed.get",
                "storage.managed.set",
                "storage.managed.remove",
                "storage.managed.clear",
                "storage.managed.getBytesInUse",
                "storage.managed.getKeys",
                "storage.managed.setAccessLevel",
            ],
        ),
    ];
    let contexts = ContextMask::ALL.without(ContextKind::ExternalPage);
    for (area, methods) in AREAS {
        for name in methods {
            table.register(
                name,
                MethodSpec::new(
                    Gate::Requires("storage"),
                    contexts,
                    move |host: &mut S, invocation| {
                        let result = handle(host, &invocation, area);
                        invocation.reply(result)
                    },
                ),
            )?;
        }
    }
    Ok(())
}
