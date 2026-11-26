use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use crate::guard;
use crate::route::Route;

/// The test-only policy: plain HTTP to 127.0.0.1 for loopback servers, and
/// the product guard for everything else. Names resolve from a fixed table
/// through the product's own answer vetting, never through the system.
#[derive(Default)]
pub(crate) struct Fixture {
    pub(crate) names: Mutex<HashMap<String, Vec<IpAddr>>>,
    pub(crate) lookups: Mutex<Vec<String>>,
    pub(crate) system: Mutex<Option<Route>>,
}

impl Fixture {
    pub(crate) fn admits(&self, url: &url::Url) -> bool {
        let loopback = url.scheme() == "http"
            && url.host() == Some(url::Host::Ipv4(Ipv4Addr::LOCALHOST))
            && url.username().is_empty()
            && url.password().is_none()
            && url.as_str().len() <= guard::MAX_URL_BYTES;
        loopback || guard::public_target(url)
    }

    pub(crate) fn system_route(&self) -> Route {
        self.system
            .lock()
            .map(|route| route.clone().unwrap_or(Route::Direct))
            .unwrap_or(Route::Unsupported)
    }

    pub(crate) fn resolver(self: &Arc<Self>) -> Arc<dyn reqwest::dns::Resolve> {
        Arc::new(FixtureResolver(self.clone()))
    }
}

struct FixtureResolver(Arc<Fixture>);

impl reqwest::dns::Resolve for FixtureResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let name = name.as_str().to_owned();
        if let Ok(mut lookups) = self.0.lookups.lock() {
            lookups.push(name.clone());
        }
        let answer = self
            .0
            .names
            .lock()
            .ok()
            .and_then(|names| names.get(&name).cloned())
            .unwrap_or_default();
        Box::pin(async move {
            Ok(guard::public_addresses(
                answer.into_iter().map(|ip| SocketAddr::new(ip, 0)),
            )?)
        })
    }
}
