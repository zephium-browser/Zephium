use url::Url;

/// Origin equality is compared as URLs, not strings, so a default port and an
/// explicit `:443` are the same authority.
pub fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}
