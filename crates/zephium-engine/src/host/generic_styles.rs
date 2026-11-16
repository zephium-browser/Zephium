//! Bounded generic matching, pulled by native and resolved on the style worker.
//! A visible page owns at most one asynchronous request; DOM mutations resolve
//! it. Unchanged or hidden pages do not cause periodic native/renderer work.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod supported {
    use super::super::content_styles::{DocumentStyleState, StyleKey};
    use super::super::dispatch::with_generic_style;
    use super::super::{permits::EventPermit, EngineHost};
    use crate::navigation_epoch::NavigationEpochTracker;
    use std::sync::{Arc, Mutex};
    use zephium_core::ids::ItemId;

    struct Pull {
        id: ItemId,
        state: Arc<DocumentStyleState>,
        key: StyleKey,
        fingerprint: String,
        visibility_revision: u64,
        charged_bytes: usize,
        token: String,
        navigation: NavigationEpochTracker,
        permit: EventPermit,
        provider: Arc<dyn zephium_core::blocker::DocumentStyleProvider>,
    }
    impl Drop for Pull {
        fn drop(&mut self) {
            super::super::content_styles::release_script(self.charged_bytes);
            self.state.end_generic();
        }
    }
    impl Pull {
        fn attributed(&self, host: &EngineHost) -> bool {
            host.views.get(&self.id).is_some_and(|view| {
                super::super::permits::navigation_callback_matches(
                    &view.event_permit,
                    &view.navigation,
                    &self.permit,
                    &self.navigation,
                    self.key.epoch,
                ) && self
                    .navigation
                    .matches_committed_snapshot(self.key.epoch, &self.key.url)
                    && self.state.generic_current(&self.key, &self.fingerprint)
            })
        }
    }
    struct Batch {
        token: String,
        url: String,
        subscription: String,
        serial: u64,
        tokens: Vec<String>,
    }
    fn decode(text: &str) -> Option<Batch> {
        if text.len() > 128 * 1024 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let object = value.as_object()?;
        if object.len() != 5 {
            return None;
        }
        let tokens = object.get("tokens")?.as_array()?;
        if tokens.len() > 256 {
            return None;
        }
        let batch = Batch {
            token: object.get("token")?.as_str()?.to_owned(),
            url: object.get("url")?.as_str()?.to_owned(),
            subscription: object.get("subscription")?.as_str()?.to_owned(),
            serial: object.get("serial")?.as_u64()?,
            tokens: tokens
                .iter()
                .map(|token| token.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
        };
        if batch.token.len() != 32
            || !batch.token.bytes().all(|b| b.is_ascii_hexdigit())
            || batch.url.len() > 32768
            || batch.subscription.len() != 64
            || !batch.subscription.bytes().all(|b| b.is_ascii_hexdigit())
            || batch.serial == 0
            || batch.serial > 9_007_199_254_740_991
            || batch.tokens.is_empty()
            || batch.tokens.len() > 256
            || batch.tokens.iter().map(String::len).sum::<usize>() > 65536
            || batch.tokens.iter().any(|key| {
                key.len() < 2
                    || key.len() > 4097
                    || !matches!(key.as_bytes()[0], b'.' | b'#')
                    || !key.as_bytes()[1..]
                        .iter()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
        {
            return None;
        }
        Some(batch)
    }
    impl EngineHost {
        fn generic_visible(&self, id: ItemId) -> bool {
            self.stages.values().any(|stage| stage.wants_visible(id))
        }
        pub(crate) fn refresh_generic_styles(&mut self, id: ItemId) {
            let visible = self.generic_visible(id);
            if let Some(view) = self.views.get(&id) {
                view.content_styles.generic_visibility(visible);
            }
            if !visible {
                return;
            }
            let Some(profile) = self.partitions.get(&id).map(|p| p.profile()) else {
                return;
            };
            if self.erasure_tombstones.contains(&profile) {
                return;
            }
            let Some(view) = self.views.get(&id).filter(|view| view.presentable) else {
                return;
            };
            let Some(provider) = self
                .content_policies
                .get(&profile)
                .and_then(|p| p.applied.as_ref())
                .and_then(|p| p.cosmetics.clone())
            else {
                return;
            };
            let state = view.content_styles.clone();
            let Some((key, fingerprint, token, visibility_revision)) = state.begin_generic() else {
                return;
            };
            if key.subscription != Some(provider.fingerprint())
                || !view
                    .navigation
                    .matches_committed_snapshot(key.epoch, &key.url)
            {
                state.end_generic();
                return;
            }
            let pull = Pull {
                id,
                state,
                key,
                fingerprint,
                visibility_revision,
                charged_bytes: 0,
                token,
                navigation: view.navigation.clone(),
                permit: view.event_permit.clone(),
                provider,
            };
            let params = serde_json::json!([pull.token, pull.key.url, pull.fingerprint]);
            let expression = format!("((p)=>{{const a=globalThis.__zephium_content_style_v1__;return a?a.pullGeneric(p[0],p[1],p[2]):null;}})({params})");
            let pending = Mutex::new(Some(pull));
            // The platform call never blocks its native UI thread. It returns
            // a JSON string only when new bounded tokens exist, or cancellation.
            #[cfg(not(target_os = "linux"))]
            use crate::platform::cosmetic_pull::evaluate;
            #[cfg(target_os = "linux")]
            use crate::platform::cosmetic_pull::evaluate_until_navigation as evaluate;
            // Generic pulls wait for document mutations, not a wall-clock deadline.
            // Page visibility/policy changes settle the JS waiter; navigation and
            // destruction also retire the Linux native evaluation.
            let _ = evaluate(&view.view, &expression, move |result| {
                let Some(pull) = pending.lock().unwrap_or_else(|p| p.into_inner()).take() else {
                    return;
                };
                let _ = with_generic_style(id, move |host| host.generic_tokens(pull, result));
            });
        }
        pub(crate) fn refresh_visible_generic_styles(&mut self) {
            let ids: Vec<_> = self.views.keys().copied().collect();
            for id in ids {
                self.refresh_generic_styles(id);
            }
        }
        fn generic_tokens(&mut self, pull: Pull, result: Option<String>) {
            if !pull.attributed(self) {
                let id = pull.id;
                drop(pull);
                self.refresh_generic_styles(id);
                return;
            }
            if !self.generic_visible(pull.id) {
                return;
            }
            let Some(text) = result else {
                let id = pull.id;
                let changed = pull.state.generic_visibility(true) != pull.visibility_revision;
                drop(pull);
                if changed {
                    self.refresh_generic_styles(id);
                }
                return;
            };
            let Some(worker) = &self.style_worker else {
                return;
            };
            worker.submit(move || {
                let mut pull = pull;
                if !pull.navigation.matches_committed_snapshot(pull.key.epoch, &pull.key.url)
                    || !pull.state.generic_current(&pull.key, &pull.fingerprint) { return None; }
                let batch = decode(&text)?;
                if batch.token != pull.token || batch.url != pull.key.url
                    || batch.subscription != pull.fingerprint { return None; }
                let selectors = pull.provider.generic_selectors(&pull.key.url, &batch.tokens).ok()?;
                let params = serde_json::json!([pull.token, pull.key.url, pull.fingerprint, batch.serial, selectors]);
                let script = format!("((p)=>{{const a=globalThis.__zephium_content_style_v1__;return !!a&&a.applyGeneric(p[0],p[1],p[2],p[3],p[4]);}})({params})");
                if script.len() > 2 * 1024 * 1024 { return None; }
                pull.charged_bytes = super::super::content_styles::charge_script(script.len())?;
                Some(Box::new(move || { let id = pull.id;
                    let _ = with_generic_style(id, move |host| host.deliver_generic(pull, script));
                }))
            });
        }
        fn deliver_generic(&mut self, pull: Pull, script: String) {
            let id = pull.id;
            if !pull.attributed(self) {
                drop(pull);
                self.refresh_generic_styles(id);
                return;
            }
            if !self.generic_visible(id) {
                return;
            }
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let pending = Mutex::new(Some(pull));
            let _ = view.evaluate_script_with_callback(&script, move |result| {
                let Some(pull) = pending.lock().unwrap_or_else(|p| p.into_inner()).take() else {
                    return;
                };
                let _ = with_generic_style(id, move |host| {
                    let current = pull.attributed(host);
                    drop(pull);
                    if result == "true" || !current {
                        host.refresh_generic_styles(id);
                    }
                });
            });
        }
    }
    #[cfg(all(test, target_os = "linux"))]
    mod native_tests {
        use super::decode;
        use gtk::prelude::*;
        use std::sync::mpsc::{self, Receiver};
        use std::time::{Duration, Instant};
        use webkit2gtk::WebViewExt;
        use wry::{WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};

        fn wait<T>(receiver: Receiver<T>) -> T {
            let context = gtk::glib::MainContext::default();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(value) = receiver.try_recv() {
                    return value;
                }
                assert!(
                    Instant::now() < deadline,
                    "native generic operation did not settle"
                );
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn evaluate(view: &wry::WebView, expression: &str) -> String {
            let (sent, received) = mpsc::channel();
            assert!(crate::platform::cosmetic_pull::evaluate(
                view,
                expression,
                move |value| {
                    let _ = sent.send(value);
                }
            ));
            wait(received).expect("fixed native fixture evaluation failed")
        }

        fn call(view: &wry::WebView, method: &str, arguments: serde_json::Value) -> String {
            evaluate(
                view,
                &format!(
                "JSON.stringify(globalThis.__zephium_content_style_v1__.{method}(...{arguments}))"
            ),
            )
        }

        fn pull(
            view: &wry::WebView,
            token: &str,
            url: &str,
            fingerprint: &str,
        ) -> Receiver<Option<String>> {
            let arguments = serde_json::json!([token, url, fingerprint]);
            let expression =
                format!("globalThis.__zephium_content_style_v1__.pullGeneric(...{arguments})");
            let (sent, received) = mpsc::channel();
            assert!(crate::platform::cosmetic_pull::evaluate_until_navigation(
                view,
                &expression,
                move |value| {
                    let _ = sent.send(value);
                }
            ));
            received
        }

        fn apply(view: &wry::WebView, encoded: &str, selectors: &[&str]) -> String {
            let batch = decode(encoded).expect("native token batch satisfies host bounds");
            call(
                view,
                "applyGeneric",
                serde_json::json!([
                    batch.token,
                    batch.url,
                    batch.subscription,
                    batch.serial,
                    selectors
                ]),
            )
        }

        fn load(view: &wry::WebView, html: &str) {
            let page = view.webview();
            let (sent, received) = mpsc::channel();
            let handler = page.connect_load_changed(move |_, event| {
                if event == webkit2gtk::LoadEvent::Finished {
                    let _ = sent.send(());
                }
            });
            page.load_html(html, Some("https://generic-fixture.invalid/"));
            wait(received);
            page.disconnect(handler);
        }

        struct Close(gtk::Window);
        impl Drop for Close {
            fn drop(&mut self) {
                self.0.close();
            }
        }

        #[test]
        #[ignore = "requires isolated Xvfb and admitted WebKitGTK"]
        fn linux_generic_pull_handles_dynamic_dom_pause_navigation_and_visibility() {
            crate::platform::linux::enforce_runtime_security_floor().unwrap();
            gtk::init().unwrap();
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            let _close = Close(window.clone());
            window.set_default_size(480, 320);
            let container = gtk::Fixed::new();
            window.add(&container);
            let mut context = wry::WebContext::new_ephemeral().unwrap();
            let view = WebViewBuilder::new_with_web_context(&mut context)
                .with_incognito(true)
                .with_initialization_script(include_str!("content_style.js"))
                .build_gtk(&container)
                .unwrap();
            window.show_all();
            load(
                &view,
                "<!doctype html><div class='advert'>ad</div><div class='except'>exception</div>",
            );
            let identity: serde_json::Value = serde_json::from_str(&evaluate(
                &view,
                "globalThis.__zephium_content_style_v1__.inspectEncoded()",
            ))
            .unwrap();
            let token = identity["token"].as_str().unwrap();
            let url = identity["url"].as_str().unwrap();
            let fingerprint = "a".repeat(64);
            assert_eq!(
                call(
                    &view,
                    "subscription",
                    serde_json::json!([token, url, "0000000000000001", fingerprint, "", true, ""])
                ),
                "true"
            );
            let initial = wait(pull(&view, token, url, &fingerprint)).unwrap();
            let batch = decode(&initial).unwrap();
            assert!(batch.tokens.iter().any(|token| token == ".advert"));
            assert!(batch.tokens.iter().any(|token| token == ".except"));
            // Native supplies only approved selectors; exception tokens do not
            // make the page hide anything by themselves.
            assert_eq!(apply(&view, &initial, &[".advert"]), "true");
            assert_eq!(
                evaluate(
                    &view,
                    "getComputedStyle(document.querySelector('.advert')).display"
                ),
                "none"
            );
            assert_ne!(
                evaluate(
                    &view,
                    "getComputedStyle(document.querySelector('.except')).display"
                ),
                "none"
            );

            let pending = pull(&view, token, url, &fingerprint);
            assert_eq!(evaluate(&view, "'idle barrier'"), "idle barrier");
            assert!(matches!(pending.try_recv(), Err(mpsc::TryRecvError::Empty)));
            evaluate(&view, "(()=>{const e=document.createElement('div');e.className='later';document.body.append(e);return 'added'})()");
            let dynamic = wait(pending).unwrap();
            assert!(decode(&dynamic)
                .unwrap()
                .tokens
                .iter()
                .any(|token| token == ".later"));
            assert_eq!(apply(&view, &dynamic, &[".later"]), "true");
            assert_eq!(
                evaluate(
                    &view,
                    "getComputedStyle(document.querySelector('.later')).display"
                ),
                "none"
            );

            let hidden = pull(&view, token, url, &fingerprint);
            window.hide();
            assert!(
                wait(hidden).is_none(),
                "visibilitychange must clear the JS waiter"
            );
            assert_eq!(evaluate(&view, "String(document.hidden)"), "true");
            window.show_all();
            // GTK mapping reaches WebKit asynchronously. Observe the page's
            // actual visibility before issuing the replacement pull.
            assert_eq!(
                evaluate(
                    &view,
                    "new Promise(resolve=>{const shown=()=>{if(!document.hidden){document.removeEventListener('visibilitychange',shown);resolve('visible')}};document.addEventListener('visibilitychange',shown);shown()})"
                ),
                "visible"
            );
            let resumed = pull(&view, token, url, &fingerprint);
            evaluate(&view, "(()=>{const e=document.createElement('div');e.className='after_show';document.body.append(e);return 'added'})()");
            let resumed = wait(resumed).unwrap();
            assert!(decode(&resumed)
                .unwrap()
                .tokens
                .iter()
                .any(|token| token == ".after_show"));
            assert_eq!(apply(&view, &resumed, &[".after_show"]), "true");

            let paused = pull(&view, token, url, &fingerprint);
            let next = "b".repeat(64);
            assert_eq!(
                call(
                    &view,
                    "subscription",
                    serde_json::json!([token, url, "0000000000000002", next, "", false, ""])
                ),
                "true"
            );
            assert!(wait(paused).is_none());
            assert_ne!(
                evaluate(
                    &view,
                    "getComputedStyle(document.querySelector('.advert')).display"
                ),
                "none"
            );
            assert_eq!(
                apply(&view, &initial, &[".except"]),
                "false",
                "old policy reply must be refused"
            );
            assert!(wait(pull(&view, token, url, &next)).is_none());

            let next = "c".repeat(64);
            assert_eq!(
                call(
                    &view,
                    "subscription",
                    serde_json::json!([token, url, "0000000000000003", next, "", true, ""])
                ),
                "true"
            );
            let restored = wait(pull(&view, token, url, &next)).unwrap();
            assert_eq!(apply(&view, &restored, &[".advert"]), "true");
            let navigating = pull(&view, token, url, &next);
            load(
                &view,
                "<!doctype html><div class='advert'>new document</div>",
            );
            assert!(wait(navigating).is_none());
            assert_eq!(
                apply(&view, &restored, &[".advert"]),
                "false",
                "old document reply must be refused"
            );
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn token_batches_are_bounded_and_not_a_selector_or_command_channel() {
            let valid = serde_json::json!({"token":"a".repeat(32),"url":"https://example.com/",
                "subscription":"b".repeat(64),"serial":1,"tokens":[".ad","#slot"]});
            assert!(decode(&valid.to_string()).is_some());
            for tokens in [
                serde_json::json!([".ad{}"]),
                serde_json::json!(["#x\ny"]),
                serde_json::json!(vec![".x"; 257]),
                serde_json::json!([]),
            ] {
                let mut bad = valid.clone();
                bad["tokens"] = tokens;
                assert!(decode(&bad.to_string()).is_none());
            }
            let mut bad = valid;
            bad["serial"] = serde_json::json!(0);
            assert!(decode(&bad.to_string()).is_none());
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl super::EngineHost {
    pub(crate) fn refresh_generic_styles(&mut self, _: zephium_core::ids::ItemId) {}
    pub(crate) fn refresh_visible_generic_styles(&mut self) {}
}
