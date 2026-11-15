use super::*;
const ORIGIN: &str = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME: &str = "<!doctype html><script src='/own.js'></script><script>fetch('/own-data').catch(()=>{});const f=document.createElement('iframe');f.src='/own-inner.html';document.documentElement.append(f);</script>";

/// Records what WebKit tells `send-request` and the UI scheme handler about
/// extension-scheme loads from an extension frame and from its embedding page.
pub fn run(module: &ModuleDirectory, fixture: &Fixture) -> Value {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let security = context.security_manager().unwrap();
    security.register_uri_scheme_as_secure("chrome-extension");
    security.register_uri_scheme_as_cors_enabled("chrome-extension");
    let seen = Rc::new(RefCell::new(Vec::<Value>::new()));
    let recorded = seen.clone();
    context.register_uri_scheme("chrome-extension", move |request| {
        let uri = request.uri().unwrap_or_default().to_string();
        let headers = request.http_headers();
        let header = |name: &str| {
            headers
                .as_ref()
                .and_then(|headers| headers.one(name))
                .map(|value| value.to_string())
        };
        let mut recorded = recorded.borrow_mut();
        if recorded.len() < 64 {
            recorded.push(json!({
                    "uri": uri.clone(),
                    "referer": header("Referer"),
                    "origin": header("Origin"),
                    "site": header("Sec-Fetch-Site"),
                    "dest": header("Sec-Fetch-Dest"),
                    "probe": header("X-Native-Test"),
            }));
        }
        let (mime, body) = if uri.ends_with("inner.html") {
            ("text/html", &b"<!doctype html><p>inner</p>"[..])
        } else if uri.ends_with(".html") {
            ("text/html", FRAME.as_bytes())
        } else if uri.ends_with(".js") {
            (
                "text/javascript",
                &b"globalThis.nativeSchemeScript=true;"[..],
            )
        } else {
            ("text/plain", &b"extension-data"[..])
        };
        let bytes = glib::Bytes::from(body);
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        request.finish(&stream, bytes.len().try_into().unwrap(), Some(mime));
    });
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        let bytes = message.parameters().unwrap().get::<Vec<u8>>().unwrap();
        let event = decode_event(&bytes).unwrap();
        let mut events = observed.borrow_mut();
        if events.len() < 256 {
            events.push(event);
        }
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?scheme-probe", fixture.origin));
    assert!(send(&page, &UiMessage::Attach(attach(fixture)), None));
    let marked = format!("{ORIGIN}/war.html");
    let rule = |id: u32, url: String| Rule {
        id,
        url,
        method: None,
        resource_type: None,
        decision: Decision::SetRequestHeaders(vec![Header {
            name: "X-Native-Test".into(),
            value: "send-request".into(),
        }]),
    };
    let snapshot = PolicySnapshot::new(vec![
        rule(1, marked.clone()),
        rule(2, format!("{ORIGIN}/own.js")),
        rule(3, format!("{ORIGIN}/page.js")),
    ])
    .unwrap()
    .encode()
    .unwrap();
    assert!(install_policy(&page, &snapshot, true));
    evaluate(
        &page,
        &format!(
            r#"const add=(tag,src,policy)=>{{const e=document.createElement(tag);if(policy)e.referrerPolicy=policy;e.src=src;document.documentElement.append(e);}};
            add('iframe','{marked}');add('script','{ORIGIN}/page.js');add('iframe','{ORIGIN}/page-inner.html');
            add('script','{ORIGIN}/page-noref.js','no-referrer');add('iframe','{ORIGIN}/page-noref.html','no-referrer');
            await fetch('{ORIGIN}/page-data').catch(()=>{{}});await fetch('{ORIGIN}/page-nocors',{{mode:'no-cors'}}).catch(()=>{{}});
            return true;"#
        ),
        None,
    );
    pump_until(|| seen.borrow().len() >= 10);
    pump_for(Duration::from_millis(500));
    let fired: Vec<String> = events
        .borrow()
        .iter()
        .flat_map(|event| match event {
            Event::Requests { observations, .. } => observations
                .iter()
                .filter(|observation| observation.url.starts_with(ORIGIN))
                .map(|observation| observation.url.clone())
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    page.stop_loading();
    let seen = seen.borrow();
    json!({ "send_request": fired, "scheme": *seen })
}
