//! Real stdio calls prove that settings changes revoke captured approval.
use super::*;
use crate::work_connections::{store, HostFuture};
use std::sync::{Arc, Condvar, Mutex};
use zephium_core::work::WorkError;

struct Person(Box<dyn Fn() + Send + Sync>);
impl ConnectionHost for Person {
    fn record(
        &self,
        _: CallFact,
        _: bool,
        _: Option<String>,
    ) -> HostFuture<'_, Result<(), WorkError>> {
        Box::pin(async { Ok(()) })
    }
    fn ask<'a>(
        &'a self,
        _: &'a str,
        options: &'a [&'a str],
    ) -> HostFuture<'a, Result<Option<String>, WorkError>> {
        let yes = options[0].to_owned();
        Box::pin(async move { Ok(Some(yes)) })
    }
    fn confirm(&self, _: WorkConfirmV1) -> HostFuture<'_, Result<Decision, WorkError>> {
        (self.0)();
        Box::pin(async { Ok(Decision::Approved) })
    }
}

struct BlockingVault {
    value: Mutex<String>,
    entered: tokio::sync::Notify,
    release: (Mutex<bool>, Condvar),
}
impl zephium_mcp::keychain::SecretVault for BlockingVault {
    fn read(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<String, zephium_mcp::keychain::KeychainError> {
        Ok(self.value.lock().unwrap().clone())
    }
    fn write(
        &self,
        _: &str,
        _: &str,
        _: &str,
        value: &str,
    ) -> Result<(), zephium_mcp::keychain::KeychainError> {
        self.entered.notify_one();
        let ready = self.release.0.lock().unwrap();
        let _ready = self.release.1.wait_while(ready, |ready| !*ready).unwrap();
        *self.value.lock().unwrap() = value.to_owned();
        Ok(())
    }
    fn delete(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<(), zephium_mcp::keychain::KeychainError> {
        Ok(())
    }
    fn delete_server(&self, _: &str, _: &str) -> Result<(), zephium_mcp::keychain::KeychainError> {
        Ok(())
    }
}

#[tokio::test]
async fn settings_changes_revoke_captured_mcp_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    store::install(directory.path());
    let store = store::shared().unwrap();
    let pool = crate::work_connections::helper::ConnectionToolSet::default();
    let script = directory.path().join("server.py");
    let calls = directory.path().join("calls");
    std::fs::write(&script, r#"import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if 'id' not in request: continue
    if method == 'initialize': result = {'protocolVersion':'2025-11-25','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    elif method == 'tools/list': result = {'tools':[{'name':'send_note','description':'send','inputSchema':{'type':'object'}}]}
    elif method == 'tools/call':
        with open(sys.argv[1], 'a') as calls: calls.write('called\n')
        result = {'content':[{'type':'text','text':'sent'}]}
    else:
        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32601,'message':'Method not found'}}), flush=True)
        continue
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
"#).unwrap();
    let server = WorkServerV1 {
        id: "fixture".into(),
        name: "Fixture".into(),
        enabled: true,
        transport: WorkServerTransportV1::Stdio {
            command: "python3".into(),
            args: vec![
                "-u".into(),
                script.to_string_lossy().into(),
                calls.to_string_lossy().into(),
            ],
            env: vec![],
        },
    };
    for change in ["destination", "disable", "remove", "account"] {
        store.put("lifecycle", server.clone(), None).unwrap();
        let connection = pool.connection("lifecycle", "fixture").await.unwrap();
        let mut changed = server.clone();
        match change {
            "destination" => {
                if let WorkServerTransportV1::Stdio { args, .. } = &mut changed.transport {
                    args[2] = directory
                        .path()
                        .join("other-account-calls")
                        .to_string_lossy()
                        .into();
                }
            }
            "disable" => changed.enabled = false,
            _ => {}
        }
        let person = Person(Box::new(move || {
            if change == "remove" {
                store.remove("lifecycle", "fixture").unwrap();
            } else if change == "account" {
                drop(store.begin_change("lifecycle", "fixture"));
            } else {
                store.put("lifecycle", changed.clone(), None).unwrap();
            }
        }));
        let result = connection
            .call(&person, "fixture__send_note", &json!({"body":"hi"}))
            .await;
        assert!(
            result.is_error,
            "{change}: obsolete confirmation sent the call: {}",
            result.content
        );
        assert!(!calls.exists(), "{change}: the old server received a call");
        store.put("lifecycle", server.clone(), None).unwrap();
        let fresh = pool.connection("lifecycle", "fixture").await.unwrap();
        assert!(
            !Arc::ptr_eq(&connection, &fresh),
            "{change}: the pool reused obsolete authority"
        );
    }
    // An OAuth browser flow can stay open while all existing sessions remain usable.
    // Merely constructing/loading its token store is not an account mutation.
    let signed_in = pool.connection("lifecycle", "fixture").await.unwrap();
    let tokens = store.track_tokens(
        "lifecycle",
        "fixture",
        Arc::new(zephium_mcp::oauth::MemoryTokens::default()),
    );
    assert!(tokens.load().is_none());
    assert!(signed_in.is_current());
    assert!(Arc::ptr_eq(
        &signed_in,
        &pool.connection("lifecycle", "fixture").await.unwrap()
    ));
    assert!(tokens.save("new account"));
    assert!(!signed_in.is_current());

    // Hold an actual credential write open: no concurrent pool read may capture
    // the pre-change account under the new epoch and survive the completed write.
    store.put("lifecycle", server.clone(), None).unwrap();
    let before = pool.connection("lifecycle", "fixture").await.unwrap();
    let vault = Arc::new(BlockingVault {
        value: Mutex::new("old-account".into()),
        entered: tokio::sync::Notify::new(),
        release: (Mutex::new(false), Condvar::new()),
    });
    let mut unrelated = server.clone();
    unrelated.id = "unrelated".into();
    store.put("lifecycle", unrelated.clone(), None).unwrap();
    store.put("otherprofile", server.clone(), None).unwrap();
    let unrelated_connection = pool.connection("lifecycle", "unrelated").await.unwrap();
    let other_profile = pool.connection("otherprofile", "fixture").await.unwrap();
    let writer = vault.clone();
    let updated = server.clone();
    let save = tokio::task::spawn_blocking(move || {
        let secrets = [("bearer".into(), "new-account".into())]
            .into_iter()
            .collect();
        store.commit_verified("lifecycle", updated, None, &secrets, writer.as_ref())
    });
    vault.entered.notified().await;
    let unrelated_still_current = unrelated_connection.is_current();
    let other_profile_still_current = other_profile.is_current();
    assert!(!before.is_current());
    assert!(pool.connection("lifecycle", "fixture").await.is_err());
    *vault.release.0.lock().unwrap() = true;
    vault.release.1.notify_one();
    save.await.unwrap().unwrap();
    assert!(
        unrelated_still_current,
        "credential write revoked an unrelated server"
    );
    assert!(
        other_profile_still_current,
        "credential write revoked an unrelated profile"
    );
    assert_eq!(*vault.value.lock().unwrap(), "new-account");
    let after = pool.connection("lifecycle", "fixture").await.unwrap();
    assert!(!Arc::ptr_eq(&before, &after));

    // A fresh connection/approval still works after the edits.
    store.put("lifecycle", server.clone(), None).unwrap();
    let connection = McpConnection::open("lifecycle", server).await.unwrap();
    assert!(
        !connection
            .call(&Person(Box::new(|| {})), "fixture__send_note", &json!({}))
            .await
            .is_error
    );
    assert_eq!(std::fs::read_to_string(calls).unwrap(), "called\n");
}
