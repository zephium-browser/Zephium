use super::*;

fn youtube_program() -> String {
    let queries: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../scripts/blocker-scriptlets/edit-queries.json"
    ))
    .unwrap();
    let edits = queries["accepted"].as_array().unwrap()[..7]
        .iter()
        .map(|query| {
            format!(
                "www.youtube.com##+js(trusted-edit-inbound-object, JSON.stringify, 0, {})",
                query.as_str().unwrap().replace(',', "\\,")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let builtin = include_str!("../../../zephium-blocker/src/zephium_rules.txt");
    let policy = zephium_blocker::CosmeticPolicy::compile_with_trust([
        (edits.as_str(), zephium_blocker::ScriptletTrust::Trusted),
        (builtin, zephium_blocker::ScriptletTrust::Zephium),
    ])
    .unwrap();
    assert_eq!(policy.report().unknown_scriptlet_rules, 0);
    assert_eq!(policy.report().unsupported_scriptlet_rules, 0);
    assert_eq!(policy.report().dropped_player_markers, 0);
    policy.scriptlet_program().unwrap().to_owned()
}

#[test]
fn youtube_fixture_uses_the_exported_program_and_admitted_marker_edits() {
    assert!(!youtube_program().is_empty());
}

#[test]
#[ignore = "requires isolated Xvfb, disposable profiles and admitted sandboxed WebKitGTK"]
fn native_youtube_program_marks_and_edits_player_requests_offline() {
    native_init();
    let page = Page::new();
    let manager = page.view.user_content_manager().unwrap();
    let slot = ViewScriptlet::default();
    slot.replace(
        &manager,
        Some(Rc::new(Program::new(&youtube_program()).unwrap())),
    );
    // Synthetic origin only: the HTML and scripts are supplied locally and CSP denies networking.
    let html = r#"<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'"><script>
    addEventListener('error', () => document.title = 'error');
    globalThis.ytcfg = { data_: {}, get(key) { return this.data_[key]; }, set(values) { Object.assign(this.data_, values); } };
    ytcfg.set({ INNERTUBE_CONTEXT: { client: { clientName: 'WEB', userAgent: 'Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36' } } });
    const client = ytcfg.get('INNERTUBE_CONTEXT').client;
    const marked = client.userAgent.includes('; lactmilli');
    const request = {
        context: { client: { ...client } },
        attestationRequest: { omitBotguardData: true }, videoId: 'offline-fixture',
        playbackContext: { contentPlaybackContext: { lactMilliseconds: '-1', referer: 'https://www.youtube.com/' } }
    };
    const before = Date.now();
    const result = JSON.parse(JSON.stringify(request));
    const playback = result.playbackContext.contentPlaybackContext;
    const checks = [marked, result.params === '8AUB', playback.referer.endsWith('#reloadxhr'), Number(playback.lactMilliseconds) >= before, result.videoId === 'offline-fixture'];
    document.title = checks.every(Boolean) ? 'player-edits-pass' : checks.join(':');
    </script>"#;
    page.view.load_html(
        html,
        Some("https://www.youtube.com/watch?v=offline-fixture"),
    );
    pump_until(|| page.view.title().as_deref() == Some("player-edits-pass"));
    slot.replace(&manager, None);
    page.view.load_html(
        html,
        Some("https://www.youtube.com/watch?v=offline-fixture"),
    );
    pump_until(|| page.view.title().as_deref() == Some("false:false:false:false:true"));
}
