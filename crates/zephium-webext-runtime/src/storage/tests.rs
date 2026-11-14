use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::api::ArgumentValidator;
use crate::context::tests::{background_sender, content_sender, install, profile};
use crate::dispatch::{ApiTable, Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use zephium_webext::manifest::Manifest;

fn memory() -> Storage {
    Storage::memory(profile(), install(), false)
}

fn properties(value: Value) -> Items {
    value.as_object().expect("object").clone()
}

fn set(
    store: &mut Storage,
    area: Area,
    values: Value,
) -> Result<Option<StorageChangeEvent>, StorageError> {
    store.mutate(area, &Mutation::Set(properties(values)), false, 0)
}

#[test]
fn defaults_select_top_level_values_without_merging_objects() {
    let mut store = memory();
    set(
        &mut store,
        Area::Local,
        json!({"x": null,"obj":{"present":1},"other":5}),
    )
    .unwrap();
    assert_eq!(
        store.get(Area::Local, &GetKeys::All).unwrap(),
        properties(json!({"x":null,"obj":{"present":1},"other":5}))
    );
    assert_eq!(
        store.get(Area::Local, &GetKeys::Keys(vec![])).unwrap(),
        Items::new()
    );
    assert_eq!(
        store
            .get(
                Area::Local,
                &GetKeys::Defaults(properties(json!({"x":5,"missing":3,"obj":{"absent":2}})))
            )
            .unwrap(),
        properties(json!({"x":null,"missing":3,"obj":{"present":1}}))
    );
    assert_eq!(store.get_keys(Area::Local).unwrap(), ["obj", "other", "x"]);
}

#[test]
fn committed_changes_distinguish_missing_null_and_unchanged() {
    let mut store = memory();
    let changed = set(&mut store, Area::Local, json!({"x":null}))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&changed.changes).unwrap(),
        json!({"x":{"newValue":null}})
    );
    assert!(set(&mut store, Area::Local, json!({"x":null}))
        .unwrap()
        .is_none());
    let changed = set(&mut store, Area::Local, json!({"x":3}))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&changed.changes).unwrap(),
        json!({"x":{"oldValue":null,"newValue":3}})
    );
    let changed = store
        .mutate(
            Area::Local,
            &Mutation::Remove(vec!["x".into(), "x".into(), "missing".into()]),
            false,
            0,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&changed.changes).unwrap(),
        json!({"x":{"oldValue":3}})
    );
    assert!(store
        .mutate(Area::Local, &Mutation::Clear, false, 0)
        .unwrap()
        .is_none());
}

#[test]
fn chrome_quota_counts_utf8_raw_top_level_keys_and_escaped_json() {
    assert_eq!(item_bytes("a", &json!(1)), 2);
    assert_eq!(item_bytes("\"", &json!("x")), 4);
    assert_eq!(item_bytes("é", &json!("🙂")), 8);
    assert_eq!(item_bytes("", &json!("<\u{2028}\u{2029}\n\"\\\u{0}")), 32);
    assert_eq!(item_bytes("", &json!({"<": [true, null]})), 22);
    assert_eq!(item_bytes("", &json!(1.0)), 3);
    assert_eq!(item_bytes("", &json!(2_147_483_648_i64)), 12);
    assert_eq!(item_bytes("", &json!(1e20)), 23);
    assert_eq!(item_bytes("", &json!(1e21)), 5);
    assert_eq!(item_bytes("", &json!(1e-7)), 4);
    assert_eq!(item_bytes("", &json!(1e-6)), 8);
    assert_eq!(item_bytes("", &json!(-0.0)), 4);
}

#[test]
fn bytes_in_use_obeys_empty_missing_and_duplicate_keys() {
    let mut store = memory();
    set(&mut store, Area::Local, json!({"a":"x","b":null})).unwrap();
    assert_eq!(store.get_bytes_in_use(Area::Local, None).unwrap(), 9);
    assert_eq!(store.get_bytes_in_use(Area::Local, Some(&[])).unwrap(), 0);
    assert_eq!(
        store
            .get_bytes_in_use(
                Area::Local,
                Some(&["a".into(), "a".into(), "missing".into()])
            )
            .unwrap(),
        8
    );
}

#[test]
fn each_area_is_isolated_and_managed_is_empty_and_read_only() {
    let mut store = memory();
    for (area, value) in [(Area::Local, 1), (Area::Sync, 2), (Area::Session, 3)] {
        set(&mut store, area, json!({"x":value})).unwrap();
    }
    assert_eq!(store.get(Area::Local, &GetKeys::All).unwrap()["x"], 1);
    assert_eq!(store.get(Area::Sync, &GetKeys::All).unwrap()["x"], 2);
    assert_eq!(store.get(Area::Session, &GetKeys::All).unwrap()["x"], 3);
    assert!(store.get(Area::Managed, &GetKeys::All).unwrap().is_empty());
    for mutation in [
        Mutation::Set(Items::new()),
        Mutation::Remove(vec![]),
        Mutation::Clear,
    ] {
        assert_eq!(
            store.mutate(Area::Managed, &mutation, true, 0),
            Err(StorageError::ReadOnly)
        );
    }
}

#[test]
fn sync_item_quota_is_inclusive_and_bulk_rejection_is_atomic() {
    let mut store = memory();
    set(&mut store, Area::Sync, json!({"a":"x".repeat(8189)})).unwrap();
    assert_eq!(
        store.get_bytes_in_use(Area::Sync, None).unwrap(),
        QUOTA_BYTES_PER_ITEM
    );
    assert_eq!(
        set(&mut store, Area::Sync, json!({"a":0,"z":"x".repeat(8190)})),
        Err(StorageError::QuotaExceeded("Resource::kQuotaBytesPerItem"))
    );
    assert_eq!(
        store.get_bytes_in_use(Area::Sync, None).unwrap(),
        QUOTA_BYTES_PER_ITEM
    );
    assert!(!store
        .get(Area::Sync, &GetKeys::All)
        .unwrap()
        .contains_key("z"));
}

#[test]
fn sync_total_and_item_count_limits_are_independent() {
    let mut store = memory();
    let values: Items = (0..512)
        .map(|index| (format!("{index:03}"), Value::Null))
        .collect();
    store
        .mutate(Area::Sync, &Mutation::Set(values), false, 0)
        .unwrap();
    assert_eq!(
        set(&mut store, Area::Sync, json!({"extra":null})),
        Err(StorageError::QuotaExceeded("Resource::kMaxItems"))
    );
    assert_eq!(store.get_keys(Area::Sync).unwrap().len(), 512);
    store
        .mutate(Area::Sync, &Mutation::Clear, false, 0)
        .unwrap();
    let values: Items = (0..12)
        .map(|index| (format!("{index:03}"), Value::String("x".repeat(8187))))
        .collect();
    store
        .mutate(Area::Sync, &Mutation::Set(values), false, 0)
        .unwrap();
    set(&mut store, Area::Sync, json!({"end":"x".repeat(4091)})).unwrap();
    assert_eq!(
        store.get_bytes_in_use(Area::Sync, None).unwrap(),
        QUOTA_BYTES
    );
    assert_eq!(
        set(&mut store, Area::Sync, json!({"end":"x".repeat(4092)})),
        Err(StorageError::QuotaExceeded("Resource::kQuotaBytes"))
    );
}

#[test]
fn unlimited_storage_raises_local_only_and_preserves_runtime_ceiling() {
    let mut store = memory();
    let mutation = Mutation::Set(properties(
        json!({"x":"v".repeat(STORAGE_LOCAL_QUOTA_BYTES as usize)}),
    ));
    assert!(matches!(
        store.mutate(Area::Local, &mutation, false, 0),
        Err(StorageError::QuotaExceeded(_))
    ));
    store.mutate(Area::Local, &mutation, true, 0).unwrap();
    assert!(matches!(
        store.mutate(Area::Session, &mutation, true, 0),
        Err(StorageError::QuotaExceeded(_))
    ));
    assert!(matches!(
        store.mutate(Area::Sync, &mutation, true, 0),
        Err(StorageError::QuotaExceeded(_))
    ));
    assert!(matches!(
        super::quota::prepare(
            &Items::new(),
            &Mutation::Set(properties(json!({"x":0}))),
            Area::Local,
            1
        ),
        Err(StorageError::QuotaExceeded(_))
    ));
}

#[test]
fn sync_rates_count_noops_failures_and_all_mutation_kinds() {
    let mut store = memory();
    for index in 0..120 {
        let mutation = match index % 3 {
            0 => Mutation::Set(Items::new()),
            1 => Mutation::Clear,
            _ => Mutation::Remove(vec![]),
        };
        assert!(store.mutate(Area::Sync, &mutation, true, 0).is_ok());
    }
    assert_eq!(
        store.mutate(Area::Sync, &Mutation::Clear, false, 60_000_000_000),
        Err(StorageError::WriteRateExceeded(
            "MAX_WRITE_OPERATIONS_PER_MINUTE"
        ))
    );
    assert!(store
        .mutate(Area::Sync, &Mutation::Clear, false, 60_000_000_001)
        .is_ok());
    assert!(store
        .mutate(Area::Local, &Mutation::Clear, false, 0)
        .is_ok());
    assert!(store.get(Area::Sync, &GetKeys::All).is_ok());
    let mut store = memory();
    for _ in 0..120 {
        let _ = set(&mut store, Area::Sync, json!({"x":"a".repeat(8192)}));
    }
    assert!(matches!(
        set(&mut store, Area::Sync, json!({})),
        Err(StorageError::WriteRateExceeded(_))
    ));
}

#[test]
fn sync_hour_rate_does_not_reset_with_minute_window() {
    let mut store = memory();
    for minute in 0..15 {
        for _ in 0..120 {
            store
                .mutate(Area::Sync, &Mutation::Clear, false, minute * 60_000_000_001)
                .unwrap();
        }
    }
    assert_eq!(
        store.mutate(Area::Sync, &Mutation::Clear, false, 15 * 60_000_000_001),
        Err(StorageError::WriteRateExceeded(
            "MAX_WRITE_OPERATIONS_PER_HOUR"
        ))
    );
    store
        .mutate(Area::Sync, &Mutation::Clear, false, 3_600_000_000_001)
        .unwrap();
}

#[test]
fn access_changes_require_trust_and_data_access_is_checked_per_call() {
    let trusted = background_sender();
    let content = content_sender("https://example.test/");
    let mut store = memory();
    assert_eq!(
        store.check_access(&content, Area::Session),
        Err(StorageError::AccessDenied)
    );
    store
        .set_access_level(
            &trusted,
            Area::Session,
            AccessLevel::TrustedAndUntrustedContexts,
        )
        .unwrap();
    store.check_access(&content, Area::Session).unwrap();
    assert_eq!(
        store.set_access_level(&content, Area::Session, AccessLevel::TrustedContexts),
        Err(StorageError::CannotSetAccessLevel)
    );
    for area in [Area::Local, Area::Sync, Area::Session, Area::Managed] {
        store
            .set_access_level(&trusted, area, AccessLevel::TrustedContexts)
            .unwrap();
        assert_eq!(
            store.check_access(&content, area),
            Err(StorageError::AccessDenied)
        );
        store.check_access(&trusted, area).unwrap();
    }
}

#[test]
fn wrong_profile_install_and_incognito_stores_fail_closed() {
    let sender = background_sender();
    for store in [
        Storage::memory(ProfileId::from(999), install(), false),
        Storage::memory(profile(), ExtensionInstallId::from(999), false),
        Storage::memory(profile(), install(), true),
    ] {
        assert_eq!(
            store.check_access(&sender, Area::Local),
            Err(StorageError::AccessDenied)
        );
    }
}

#[test]
fn erase_clears_all_state_without_install_events() {
    let mut store = memory();
    for area in [Area::Local, Area::Sync, Area::Session] {
        set(&mut store, area, json!({"x":1})).unwrap();
    }
    store
        .set_access_level(
            &background_sender(),
            Area::Session,
            AccessLevel::TrustedAndUntrustedContexts,
        )
        .unwrap();
    store.erase().unwrap();
    for area in [Area::Local, Area::Sync, Area::Session] {
        assert!(store.get_keys(area).unwrap().is_empty());
    }
    assert_eq!(
        store.access_level(Area::Session),
        AccessLevel::TrustedContexts
    );
}

#[test]
fn sqlite_reopen_preserves_durable_areas_only_and_uninstall_removes_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("storage.sqlite");
    let mut store = Storage::new(
        profile(),
        install(),
        false,
        SqliteBackend::open(&path).unwrap(),
    )
    .unwrap();
    for area in [Area::Local, Area::Sync, Area::Session] {
        set(&mut store, area, json!({"x":null,"n":3})).unwrap();
    }
    drop(store);
    let store = Storage::new(
        profile(),
        install(),
        false,
        SqliteBackend::open(&path).unwrap(),
    )
    .unwrap();
    for area in [Area::Local, Area::Sync] {
        assert_eq!(
            store.get(area, &GetKeys::All).unwrap(),
            properties(json!({"x":null,"n":3}))
        );
    }
    assert!(store.get_keys(Area::Session).unwrap().is_empty());
    store.destroy().unwrap();
    for name in ["storage.sqlite", "storage.sqlite-wal", "storage.sqlite-shm"] {
        assert!(!temp.path().join(name).exists());
    }
}

#[test]
fn sqlite_quota_failure_is_atomic_and_erase_is_durable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("storage.sqlite");
    let mut store = Storage::new(
        profile(),
        install(),
        false,
        SqliteBackend::open(&path).unwrap(),
    )
    .unwrap();
    set(&mut store, Area::Sync, json!({"x":1})).unwrap();
    assert!(set(&mut store, Area::Sync, json!({"x":2,"z":"v".repeat(8192)})).is_err());
    assert_eq!(
        store.get(Area::Sync, &GetKeys::All).unwrap(),
        properties(json!({"x":1}))
    );
    store.erase().unwrap();
    drop(store);
    let store = Storage::new(
        profile(),
        install(),
        false,
        SqliteBackend::open(&path).unwrap(),
    )
    .unwrap();
    assert!(store.get_keys(Area::Sync).unwrap().is_empty());
}

#[test]
fn sqlite_transaction_rolls_back_on_midwrite_failure() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("storage.sqlite");
    let backend = SqliteBackend::open(&path).unwrap();
    let other = rusqlite::Connection::open(&path).unwrap();
    other.execute_batch("CREATE TRIGGER fail_write BEFORE INSERT ON storage WHEN NEW.key='z' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    let mut store = Storage::new(profile(), install(), false, backend).unwrap();
    assert_eq!(
        set(&mut store, Area::Local, json!({"a":1,"z":2})),
        Err(StorageError::Backend)
    );
    assert!(store.get_keys(Area::Local).unwrap().is_empty());
}

#[test]
fn sqlite_files_do_not_share_install_data() {
    let temp = tempfile::tempdir().unwrap();
    let mut first = Storage::new(
        profile(),
        install(),
        false,
        SqliteBackend::open(temp.path().join("a.sqlite")).unwrap(),
    )
    .unwrap();
    let second = Storage::new(
        profile(),
        ExtensionInstallId::from(20),
        false,
        SqliteBackend::open(temp.path().join("b.sqlite")).unwrap(),
    )
    .unwrap();
    set(&mut first, Area::Local, json!({"secret":"one"})).unwrap();
    assert!(second.get_keys(Area::Local).unwrap().is_empty());
}

#[test]
fn corrupt_and_future_databases_are_refused_without_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("corrupt.sqlite");
    std::fs::write(&path, b"this is not sqlite").unwrap();
    assert!(matches!(
        SqliteBackend::open(&path),
        Err(StorageError::Corrupt)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"this is not sqlite");
    let path = temp.path().join("future.sqlite");
    drop(SqliteBackend::open(&path).unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    assert!(matches!(
        SqliteBackend::open(&path),
        Err(StorageError::UnsupportedSchema)
    ));
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        99
    );
}

#[test]
fn invalid_persisted_json_and_foreign_schema_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("storage.sqlite");
    let backend = SqliteBackend::open(&path).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO storage(area,key,value) VALUES ('local','bad','[invalid')",
            [],
        )
        .unwrap();
    assert_eq!(backend.read(Area::Local), Err(StorageError::Corrupt));
    connection.execute_batch("CREATE TRIGGER foreign_trigger AFTER INSERT ON storage BEGIN DELETE FROM storage; END;").unwrap();
    drop(backend);
    assert!(matches!(
        SqliteBackend::open(&path),
        Err(StorageError::UnsupportedSchema)
    ));
}

#[test]
fn persistent_backend_is_refused_for_incognito() {
    let temp = tempfile::tempdir().unwrap();
    let backend = SqliteBackend::open(temp.path().join("storage.sqlite")).unwrap();
    assert!(matches!(
        Storage::new(profile(), install(), true, backend),
        Err(StorageError::AccessDenied)
    ));
}

#[test]
fn manual_signatures_reject_malformed_input() {
    for (method, args) in [
        ("get", json!([1])),
        ("get", json!([["a", 1]])),
        ("getBytesInUse", json!([{}])),
        ("remove", json!([null])),
        ("clear", json!([1])),
        ("getKeys", json!(["a"])),
        ("set", json!([[]])),
        ("setAccessLevel", json!([{"accessLevel":"TRUSTED"}])),
        (
            "setAccessLevel",
            json!([{"accessLevel":"TRUSTED_CONTEXTS","other":1}]),
        ),
    ] {
        assert!(ManualValidator
            .validate(&format!("storage.local.{method}"), args.as_array().unwrap())
            .is_err());
    }
    for args in [
        json!([]),
        json!([null]),
        json!(["a"]),
        json!([["a"]]),
        json!([{}]),
    ] {
        ManualValidator
            .validate("storage.local.get", args.as_array().unwrap())
            .unwrap();
    }
}

struct Host {
    storage: Storage,
    events: Vec<StorageChangeEvent>,
}

impl StorageHost for Host {
    fn storage(&mut self, _: &Sender) -> Result<&mut Storage, ApiError> {
        Ok(&mut self.storage)
    }
    fn storage_now_nanos(&self) -> u64 {
        0
    }
    fn storage_changed(&mut self, _: &Sender, change: StorageChangeEvent) {
        self.events.push(change);
    }
}

fn invoke(
    dispatcher: &mut Dispatcher<Host>,
    host: &mut Host,
    sender: &Sender,
    permission: bool,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"permissions":if permission { vec!["storage"] } else { vec![] }}),
    ));
    let permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    let args = serde_json::to_vec(&args).unwrap();
    let bytes = Envelope::Call {
        id: CallId::new(1).unwrap(),
        name: ApiName::parse(method).unwrap(),
        args: &args,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let Inbound::Reply(bytes) = dispatcher.receive(host, &permissions, sender, &bytes, 0) else {
        panic!("reply");
    };
    let Envelope::Reply { result, .. } =
        Envelope::decode(&bytes, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("reply envelope");
    };
    result
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .map_err(str::to_owned)
}

#[test]
fn registered_storage_checks_gate_access_and_routes_committed_events() {
    let mut table = ApiTable::new();
    register::<Host>(&mut table).unwrap();
    assert_eq!(table.len(), 28);
    let mut dispatcher = Dispatcher::new(table);
    let mut host = Host {
        storage: memory(),
        events: vec![],
    };
    let sender = background_sender();
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &sender,
        false,
        "storage.local.set",
        json!([{"x":1}])
    )
    .is_err());
    assert!(host.events.is_empty());
    invoke(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "storage.local.set",
        json!([{"x":1}]),
    )
    .unwrap();
    assert_eq!(host.events.len(), 1);
    assert_eq!(host.events[0].changes["x"].new_value, Some(json!(1)));
    invoke(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "storage.local.set",
        json!([{"x":1}]),
    )
    .unwrap();
    assert_eq!(host.events.len(), 1);
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &sender,
            true,
            "storage.local.get",
            json!([null])
        )
        .unwrap(),
        json!({"x":1})
    );
    let content = content_sender("https://example.test/");
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &content,
        true,
        "storage.session.get",
        json!([])
    )
    .is_err());
    invoke(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "storage.session.setAccessLevel",
        json!([{"accessLevel":"TRUSTED_AND_UNTRUSTED_CONTEXTS"}]),
    )
    .unwrap();
    invoke(
        &mut dispatcher,
        &mut host,
        &content,
        true,
        "storage.session.get",
        json!([]),
    )
    .unwrap();
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &content,
        true,
        "storage.session.setAccessLevel",
        json!([{"accessLevel":"TRUSTED_CONTEXTS"}])
    )
    .is_err());
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &sender,
        true,
        "storage.managed.clear",
        json!([])
    )
    .is_err());
    assert_eq!(host.events.len(), 1);
}

fn json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|value| json!(value)),
        ".{0,24}".prop_map(Value::String)
    ]
    .prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            proptest::collection::btree_map("[a-z]{0,8}", inner, 0..5)
                .prop_map(|values| Value::Object(values.into_iter().collect()))
        ]
    })
}

proptest! {
    #[test]
    fn quota_accounting_matches_independent_json_serialization(key in ".{0,20}", value in json_value()) {
        let serialized = serde_json::to_string(&value).unwrap().replace('<', "\\u003C").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
        prop_assert_eq!(item_bytes(&key, &value), (key.len() + serialized.len()) as u64);
    }

    #[test]
    fn storage_mutations_and_usage_match_a_reference_map(operations in proptest::collection::vec((0_u8..3, "[a-z]{0,6}", json_value()), 0..80)) {
        let mut store = memory();
        let mut model = Items::new();
        for (operation, key, value) in operations {
            let mutation = match operation {
                0 => { model.insert(key.clone(), value.clone()); Mutation::Set([(key, value)].into_iter().collect()) },
                1 => { model.remove(&key); Mutation::Remove(vec![key]) },
                _ => { model.clear(); Mutation::Clear },
            };
            store.mutate(Area::Local, &mutation, false, 0).unwrap();
            prop_assert_eq!(store.get(Area::Local, &GetKeys::All).unwrap(), model.clone());
            let total: u64 = model.iter().map(|(key,value)| item_bytes(key,value)).sum();
            prop_assert_eq!(store.get_bytes_in_use(Area::Local, None).unwrap(), total);
        }
    }
}
