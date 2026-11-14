use std::net::IpAddr;

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use url::Url;
use zephium_core::injection::MatchPattern;

use crate::envelope::ApiName;
use crate::errors::ApiError;

const MAX_FILTER_BYTES: usize = 64 * 1024;
const MAX_FILTER_ENTRIES: usize = 256;
const MAX_REGEX_BYTES: usize = 8 * 1024;
const MAX_REGEXES_PER_FILTER: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FilterCost {
    pub(super) bytes: usize,
    pub(super) regexes: usize,
}

/// Host-supplied facts used for supported event filters. Missing facts never
/// satisfy a filter that requires them; arguments are not inspected for facts.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventFacts<'a> {
    /// Committed navigation or request URL.
    pub url: Option<&'a Url>,
    /// Chrome resource type, such as `main_frame` or `xmlhttprequest`.
    pub resource_type: Option<&'a str>,
    /// Adapter-owned Chrome tab id, including `-1` for requests without a tab.
    pub tab_id: Option<i64>,
    /// Adapter-owned Chrome window id.
    pub window_id: Option<i64>,
    /// Chrome window type, such as `normal` or `popup`.
    pub window_type: Option<&'a str>,
}

/// A validated bounded event filter, compiled once when registered or restored.
#[derive(Clone, Debug)]
pub struct ListenerFilter(Filter);

#[derive(Clone, Debug)]
enum Filter {
    Any,
    Navigation(Vec<UrlFilter>),
    Request {
        urls: Vec<MatchPattern>,
        types: Option<Vec<String>>,
        tab: Option<i64>,
        window: Option<i64>,
    },
    Windows(Vec<String>),
}

#[derive(Clone, Debug)]
struct UrlFilter {
    fields: Map<String, Value>,
    regex: Option<Regex>,
    origin_regex: Option<Regex>,
    cidrs: Vec<(IpAddr, u8)>,
}

impl ListenerFilter {
    /// Validates Chrome webNavigation URL filters, webRequest request filters,
    /// and windows window-type filters. Other events reject filter arguments.
    pub fn parse(event: &str, value: Option<&Value>) -> Result<Self, ApiError> {
        Self::cost(value)?;
        if ApiName::parse(event).is_none() {
            return Err(invalid_filter());
        }
        let mut args = vec![serde_json::json!(1)];
        args.extend(value.cloned());
        let normalized = crate::schema::normalize(&format!("{event}.addListener"), &args)?;
        let value = normalized.get(1).filter(|value| !value.is_null());
        let Some(value) = value else {
            return if event.starts_with("webRequest.") {
                Err(invalid_filter())
            } else {
                Ok(Self(Filter::Any))
            };
        };
        let object = value.as_object().ok_or_else(invalid_filter)?;
        let filter = if event.starts_with("webNavigation.") {
            let filters = object
                .get("url")
                .ok_or_else(invalid_filter)?
                .as_array()
                .filter(|a| a.len() <= MAX_FILTER_ENTRIES)
                .ok_or_else(invalid_filter)?;
            Filter::Navigation(
                filters
                    .iter()
                    .map(UrlFilter::parse)
                    .collect::<Result<_, _>>()?,
            )
        } else if event.starts_with("webRequest.") {
            let urls = strings(object.get("urls").ok_or_else(invalid_filter)?)?
                .into_iter()
                .map(|s| MatchPattern::parse(&s).map_err(|_| invalid_filter()))
                .collect::<Result<_, _>>()?;
            let types = object.get("types").map(strings).transpose()?;
            let integer = |name| {
                object
                    .get(name)
                    .map(|v| {
                        v.as_i64()
                            .filter(|id| *id >= -1 && *id <= i64::from(i32::MAX))
                            .ok_or_else(invalid_filter)
                    })
                    .transpose()
            };
            Filter::Request {
                urls,
                types,
                tab: integer("tabId")?,
                window: integer("windowId")?,
            }
        } else if matches!(
            event,
            "windows.onCreated" | "windows.onRemoved" | "windows.onFocusChanged"
        ) {
            let types = object
                .get("windowTypes")
                .map(strings)
                .transpose()?
                .unwrap_or_else(|| vec!["normal".into(), "popup".into()]);
            Filter::Windows(types)
        } else {
            return Err(invalid_filter());
        };
        Ok(Self(filter))
    }

    pub(super) fn cost(value: Option<&Value>) -> Result<FilterCost, ApiError> {
        let Some(value) = value else {
            return Ok(FilterCost::default());
        };
        let bytes = serde_json::to_vec(value)
            .map_err(|_| invalid_filter())?
            .len();
        if bytes > MAX_FILTER_BYTES {
            return Err(ApiError::message_too_large());
        }
        let regexes = value
            .get("url")
            .and_then(Value::as_array)
            .map_or(0, |filters| {
                filters
                    .iter()
                    .map(|filter| {
                        usize::from(filter.get("urlMatches").is_some())
                            + usize::from(filter.get("originAndPathMatches").is_some())
                    })
                    .sum()
            });
        if regexes > MAX_REGEXES_PER_FILTER {
            return Err(ApiError::too_many_pending());
        }
        Ok(FilterCost { bytes, regexes })
    }

    /// Whether adapter facts satisfy this listener's filter.
    pub fn matches(&self, facts: &EventFacts<'_>) -> bool {
        match &self.0 {
            Filter::Any => true,
            Filter::Navigation(filters) => facts
                .url
                .is_some_and(|url| filters.iter().any(|f| f.matches(url))),
            Filter::Request {
                urls,
                types,
                tab,
                window,
            } => {
                facts
                    .url
                    .is_some_and(|url| urls.iter().any(|p| p.matches_url(url)))
                    && types.as_ref().is_none_or(|types| {
                        facts
                            .resource_type
                            .is_some_and(|kind| types.iter().any(|t| t == kind))
                    })
                    && tab.is_none_or(|tab| facts.tab_id == Some(tab))
                    && window.is_none_or(|window| facts.window_id == Some(window))
            }
            Filter::Windows(types) => facts
                .window_type
                .is_some_and(|kind| types.iter().any(|t| t == kind)),
        }
    }
}

impl UrlFilter {
    fn parse(value: &Value) -> Result<Self, ApiError> {
        let fields = value.as_object().ok_or_else(invalid_filter)?;
        for (name, value) in fields {
            if matches!(name.as_str(), "urlMatches" | "originAndPathMatches") {
                value
                    .as_str()
                    .filter(|v| v.len() <= MAX_REGEX_BYTES)
                    .ok_or_else(invalid_filter)?;
            } else if name == "ports" {
                validate_ports(value)?;
            } else if matches!(name.as_str(), "schemes" | "cidrBlocks") {
                strings(value)?;
            } else if !TEXT_FIELDS.contains(&name.as_str()) {
                return Err(invalid_filter());
            }
        }
        let compile = |name| {
            fields
                .get(name)
                .and_then(Value::as_str)
                .map(|pattern| {
                    RegexBuilder::new(pattern)
                        .size_limit(64 * 1024)
                        .dfa_size_limit(32 * 1024)
                        .build()
                        .map_err(|_| invalid_filter())
                })
                .transpose()
        };
        let cidrs = fields
            .get("cidrBlocks")
            .map(strings)
            .transpose()?
            .unwrap_or_default()
            .iter()
            .map(|s| parse_cidr(s))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            fields: fields.clone(),
            regex: compile("urlMatches")?,
            origin_regex: compile("originAndPathMatches")?,
            cidrs,
        })
    }

    fn matches(&self, url: &Url) -> bool {
        let mut normalized = url.clone();
        normalized.set_fragment(None);
        let mut origin_path = normalized.clone();
        origin_path.set_query(None);
        let host = url.host_str().unwrap_or("");
        for (name, value) in &self.fields {
            if TEXT_FIELDS.contains(&name.as_str()) {
                let expected = value.as_str().unwrap_or("");
                let (component, operation) = if let Some(operation) = name.strip_prefix("host") {
                    (host, operation)
                } else if let Some(operation) = name.strip_prefix("path") {
                    (url.path(), operation)
                } else if let Some(operation) = name.strip_prefix("query") {
                    (url.query().unwrap_or(""), operation)
                } else {
                    (normalized.as_str(), name.strip_prefix("url").unwrap_or(""))
                };
                let matches = match operation {
                    "Contains" if name == "hostContains" => {
                        format!(".{component}").contains(expected)
                    }
                    "Contains" => component.contains(expected),
                    "Equals" => component == expected,
                    "Prefix" => component.starts_with(expected),
                    "Suffix" => component.ends_with(expected),
                    _ => false,
                };
                if !matches {
                    return false;
                }
            } else if name == "schemes" {
                if !value
                    .as_array()
                    .is_some_and(|values| values.iter().any(|s| s.as_str() == Some(url.scheme())))
                {
                    return false;
                }
            } else if name == "ports" {
                if !url
                    .port_or_known_default()
                    .is_some_and(|port| port_matches(value, port))
                {
                    return false;
                }
            } else if name == "cidrBlocks"
                && !url
                    .host_str()
                    .and_then(|s| s.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                    .is_some_and(|ip| {
                        self.cidrs
                            .iter()
                            .any(|(network, prefix)| in_cidr(ip, *network, *prefix))
                    })
            {
                return false;
            }
        }
        self.regex
            .as_ref()
            .is_none_or(|regex| regex.is_match(normalized.as_str()))
            && self
                .origin_regex
                .as_ref()
                .is_none_or(|regex| regex.is_match(origin_path.as_str()))
    }
}

fn strings(value: &Value) -> Result<Vec<String>, ApiError> {
    value
        .as_array()
        .filter(|a| a.len() <= MAX_FILTER_ENTRIES)
        .ok_or_else(invalid_filter)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or_else(invalid_filter))
        .collect()
}

fn validate_ports(value: &Value) -> Result<(), ApiError> {
    let list = value
        .as_array()
        .filter(|a| a.len() <= MAX_FILTER_ENTRIES)
        .ok_or_else(invalid_filter)?;
    for port in list {
        if let Some(port) = port.as_u64() {
            if port > u64::from(u16::MAX) {
                return Err(invalid_filter());
            }
        } else if let Some(range) = port.as_array() {
            if range.len() != 2 {
                return Err(invalid_filter());
            }
            let start = range[0].as_u64().ok_or_else(invalid_filter)?;
            let end = range[1].as_u64().ok_or_else(invalid_filter)?;
            if start > end || end > u64::from(u16::MAX) {
                return Err(invalid_filter());
            }
        } else {
            return Err(invalid_filter());
        }
    }
    Ok(())
}

fn port_matches(value: &Value, port: u16) -> bool {
    value.as_array().is_some_and(|values| {
        values.iter().any(|v| {
            v.as_u64() == Some(u64::from(port))
                || v.as_array().is_some_and(|range| {
                    range
                        .first()
                        .and_then(Value::as_u64)
                        .is_some_and(|start| start <= u64::from(port))
                        && range
                            .get(1)
                            .and_then(Value::as_u64)
                            .is_some_and(|end| u64::from(port) <= end)
                })
        })
    })
}

fn parse_cidr(text: &str) -> Result<(IpAddr, u8), ApiError> {
    let (ip, prefix) = text.split_once('/').ok_or_else(invalid_filter)?;
    let ip: IpAddr = ip.parse().map_err(|_| invalid_filter())?;
    let prefix: u8 = prefix.parse().map_err(|_| invalid_filter())?;
    if prefix > if ip.is_ipv4() { 32 } else { 128 } {
        return Err(invalid_filter());
    }
    Ok((ip, prefix))
}

fn in_cidr(ip: IpAddr, network: IpAddr, prefix: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(network)) => {
            prefix == 0 || (u32::from(ip) >> (32 - prefix)) == (u32::from(network) >> (32 - prefix))
        }
        (IpAddr::V6(ip), IpAddr::V6(network)) => {
            prefix == 0
                || (u128::from(ip) >> (128 - prefix)) == (u128::from(network) >> (128 - prefix))
        }
        _ => false,
    }
}

fn invalid_filter() -> ApiError {
    ApiError::other("Invalid event filter.")
}

const TEXT_FIELDS: &[&str] = &[
    "hostContains",
    "hostEquals",
    "hostPrefix",
    "hostSuffix",
    "pathContains",
    "pathEquals",
    "pathPrefix",
    "pathSuffix",
    "queryContains",
    "queryEquals",
    "queryPrefix",
    "querySuffix",
    "urlContains",
    "urlEquals",
    "urlPrefix",
    "urlSuffix",
];
