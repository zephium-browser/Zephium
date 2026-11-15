//! Declared HTML language only; this is not a body-text detector.
use super::*;
use zephium_webext_runtime::{
    context::{DocumentGeneration, FrameId},
    ApiError,
};

const WORLD: &str = "zephium-declared-language";
const READ: &str = "return (document.documentElement?.getAttribute('lang') || '').slice(0, 256);";

fn declared(value: &str) -> String {
    let hint = value.trim();
    if hint.len() > 64 || !hint.is_ascii() {
        return "und".into();
    }
    let hint = hint.replace('_', "-");
    let mut parts = hint.split('-');
    let primary = parts.next().unwrap_or("").to_ascii_lowercase();
    let region = parts.next();
    if parts.next().is_some()
        || region.is_some_and(|r| r.len() != 2 || !r.bytes().all(|b| b.is_ascii_alphabetic()))
    {
        return "und".into();
    }
    // ISO 639-1 declarations, excluding aliases that require a detector's mapping.
    const LANGUAGES: &str = "aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br bs ca ce ch co cr cs cu cv cy da de dv dz ee el en eo es et eu fa ff fi fj fo fr fy ga gd gl gn gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu ja jv ka kg ki kj kk kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se sg si sk sl sm sn so sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk ur uz ve vi vo wa wo xh yi yo za zh zu";
    if primary.len() != 2
        || !LANGUAGES
            .split_ascii_whitespace()
            .any(|language| language == primary)
    {
        return "und".into();
    }
    if primary == "zh" {
        return match region.map(str::to_ascii_uppercase).as_deref() {
            Some("CN") => "zh-CN".into(),
            Some("TW") => "zh-TW".into(),
            None => primary,
            _ => "und".into(),
        };
    }
    primary
}

fn finish(inner: &Rc<Inner>, pending: u64, result: Result<String, ApiError>) {
    let _ = inner.bridge.with_runtime(|host, out| {
        host.settle_browser(pending, BrowserSettlement::Language(result), out)
    });
    inner.schedule();
}

pub(super) fn submit(inner: &Rc<Inner>, pending: u64, tab: ItemId, document: DocumentGeneration) {
    let native = inner.browser.view_of(tab).and_then(|id| {
        inner.bridge.frame_target(id, FrameId::MAIN, document)?;
        let view = inner.tabs.borrow().get(&id)?.view.upgrade()?;
        let resource = view.main_resource()?;
        Some((id, view, resource))
    });
    let Some((id, view, resource)) = native else {
        finish(inner, pending, Err(ApiError::context_invalidated()));
        return;
    };
    let weak = Rc::downgrade(inner);
    glib::MainContext::default().spawn_local(async move {
        let query = gio::GioFuture::new(&view, |view, cancellable, done| {
            view.call_async_javascript_function(
                READ,
                None,
                Some(WORLD),
                None,
                Some(cancellable),
                move |result| done.resolve(result),
            );
        });
        let value = futures_lite::future::race(
            async {
                query
                    .await
                    .map(|value| declared(&value.to_string()))
                    .map_err(|_| ApiError::other("Cannot determine language: document unavailable"))
            },
            async {
                glib::timeout_future(Duration::from_secs(5)).await;
                Err(ApiError::other(
                    "Cannot determine language: document did not respond",
                ))
            },
        )
        .await;
        let Some(inner) = weak.upgrade().filter(|inner| !inner.retired.get()) else {
            return;
        };
        let result = if view.main_resource().as_ref() == Some(&resource)
            && inner.browser.view_of(tab) == Some(id)
            && inner
                .bridge
                .frame_target(id, FrameId::MAIN, document)
                .is_some()
        {
            value
        } else {
            Err(ApiError::context_invalidated())
        };
        finish(&inner, pending, result);
    });
}

#[cfg(test)]
mod tests {
    use super::declared;
    #[test]
    fn declarations_are_bounded_and_never_inferred_from_unknown_text() {
        for (hint, expected) in [
            ("fr", "fr"),
            (" EN_us ", "en"),
            ("ZH_tw", "zh-TW"),
            ("ja", "ja"),
            ("", "und"),
            ("???", "und"),
            ("zz", "und"),
            ("fr,es", "und"),
            ("zh-Hant", "und"),
            ("en-US-extra", "und"),
            ("english", "und"),
        ] {
            assert_eq!(declared(hint), expected);
        }
        assert_eq!(declared(&"en".repeat(100)), "und");
    }
}
