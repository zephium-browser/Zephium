//! Read-only policy facts from the invoking profile's live native views.
use super::*;
use webkit2gtk::{WebContextExt, WebsiteDataManagerExt};
use zephium_webext_runtime::{
    api::privacy::FormSetting, context::Sender, dispatch::Responder, ApiError,
};

fn read(inner: &Inner, setting: FormSetting) -> Result<bool, ApiError> {
    let manager = inner
        .context
        .website_data_manager()
        .ok_or_else(zephium_webext_runtime::browser::unsupported)?;
    let views: Vec<_> = inner
        .tabs
        .borrow()
        .values()
        .filter_map(|tab| tab.view.upgrade())
        .collect();
    if views.is_empty()
        || views.iter().any(|view| {
            view.context().as_ref() != Some(&inner.context)
                || view.website_data_manager().as_ref() != Some(&manager)
        })
    {
        return Err(zephium_webext_runtime::browser::unsupported());
    }
    match setting {
        FormSetting::ThirdPartyCookies if manager.is_itp_enabled() => Ok(false),
        FormSetting::HyperlinkAuditing => {
            let mut actual = None;
            for view in views {
                let settings = view
                    .settings()
                    .ok_or_else(zephium_webext_runtime::browser::unsupported)?;
                let value = settings.property::<bool>("enable-hyperlink-auditing");
                if actual.is_some_and(|previous| previous != value) {
                    return Err(zephium_webext_runtime::browser::unsupported());
                }
                actual = Some(value);
            }
            actual.ok_or_else(zephium_webext_runtime::browser::unsupported)
        }
        FormSetting::NetworkPrediction => {
            for view in views {
                let settings = view
                    .settings()
                    .ok_or_else(zephium_webext_runtime::browser::unsupported)?;
                if crate::settings::link_prefetch_enabled(&settings) != Some(false) {
                    return Err(zephium_webext_runtime::browser::unsupported());
                }
            }
            // Zephium has no browser prediction service; native link prefetch is off.
            Ok(false)
        }
        _ => Err(zephium_webext_runtime::browser::unsupported()),
    }
}

pub(super) fn submit(
    inner: &Rc<Inner>,
    sender: Box<Sender>,
    setting: FormSetting,
    responder: Responder,
) {
    let allowed = !inner.retired.get()
        && inner
            .bridge
            .with_runtime(|host, _| host.privacy_allowed(&sender))
            .unwrap_or(false);
    let result = if allowed {
        read(inner, setting)
            .map(|value| serde_json::json!({"value":value,"levelOfControl":"not_controllable"}))
    } else {
        Err(ApiError::context_invalidated())
    };
    let _ = inner.bridge.with_runtime(|host, out| {
        let result = if host.privacy_allowed(&sender) {
            result
        } else {
            Err(ApiError::context_invalidated())
        };
        host.settle_call(responder, result, out);
    });
    inner.schedule();
}
