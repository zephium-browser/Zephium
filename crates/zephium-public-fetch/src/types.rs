use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Expect {
    Html,
    Xml,
    Text,
}

#[derive(Clone, Debug)]
pub struct ReadRequest {
    pub url: url::Url,
    pub expect: Expect,
    /// Most decoded bytes the caller wants; the reader's own limit still applies.
    pub max_bytes: usize,
    pub deadline: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusClass {
    Ok,
    NonAuthoritative,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub queued: Duration,
    /// Time spent opening new connections; `None` when every hop reused one.
    pub connect: Option<Duration>,
    pub first_byte: Duration,
    pub total: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicBody {
    pub final_url: url::Url,
    pub hops: u8,
    pub status: StatusClass,
    /// The declared media type essence, lowercased; `None` when it was sniffed.
    pub media_type: Option<String>,
    pub charset: Option<String>,
    pub bytes: Vec<u8>,
    pub truncated: bool,
    /// Header and body octets received, before content decoding.
    pub wire_bytes: u64,
    pub timings: Timings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimeoutPhase {
    Queue,
    Connect,
    FirstByte,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusRefusal {
    /// 401 or 403.
    Denied,
    /// 429.
    RateLimited,
    /// A response marked by a well-known bot-challenge header, whatever its status.
    Challenge,
    Client,
    Server,
    /// Informational, non-redirect 3xx, or a success status other than 200/203.
    Unexpected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SizeLimit {
    Headers,
    Wire,
}

/// Why a read produced no body. Never carries content, header values or the URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadRefusal {
    NotPublicHttps,
    RedirectRefused,
    TooManyRedirects,
    DnsRefused,
    /// The person's proxy setting cannot be reproduced exactly, so nothing is sent.
    ProxyUnsupported,
    /// The proxy setting changed while the read was in flight.
    RouteChanged,
    Timeout(TimeoutPhase),
    Status(StatusRefusal),
    WrongContentType,
    TooLarge(SizeLimit),
    /// An unsupported, stacked, oversized-window or corrupt content coding.
    Encoding,
    Transport,
}

impl ReadRefusal {
    pub const ALL: [ReadRefusal; 21] = [
        ReadRefusal::NotPublicHttps,
        ReadRefusal::RedirectRefused,
        ReadRefusal::TooManyRedirects,
        ReadRefusal::DnsRefused,
        ReadRefusal::ProxyUnsupported,
        ReadRefusal::RouteChanged,
        ReadRefusal::Timeout(TimeoutPhase::Queue),
        ReadRefusal::Timeout(TimeoutPhase::Connect),
        ReadRefusal::Timeout(TimeoutPhase::FirstByte),
        ReadRefusal::Timeout(TimeoutPhase::Body),
        ReadRefusal::Status(StatusRefusal::Denied),
        ReadRefusal::Status(StatusRefusal::RateLimited),
        ReadRefusal::Status(StatusRefusal::Challenge),
        ReadRefusal::Status(StatusRefusal::Client),
        ReadRefusal::Status(StatusRefusal::Server),
        ReadRefusal::Status(StatusRefusal::Unexpected),
        ReadRefusal::WrongContentType,
        ReadRefusal::TooLarge(SizeLimit::Headers),
        ReadRefusal::TooLarge(SizeLimit::Wire),
        ReadRefusal::Encoding,
        ReadRefusal::Transport,
    ];

    pub(crate) fn index(self) -> usize {
        match self {
            ReadRefusal::NotPublicHttps => 0,
            ReadRefusal::RedirectRefused => 1,
            ReadRefusal::TooManyRedirects => 2,
            ReadRefusal::DnsRefused => 3,
            ReadRefusal::ProxyUnsupported => 4,
            ReadRefusal::RouteChanged => 5,
            ReadRefusal::Timeout(TimeoutPhase::Queue) => 6,
            ReadRefusal::Timeout(TimeoutPhase::Connect) => 7,
            ReadRefusal::Timeout(TimeoutPhase::FirstByte) => 8,
            ReadRefusal::Timeout(TimeoutPhase::Body) => 9,
            ReadRefusal::Status(StatusRefusal::Denied) => 10,
            ReadRefusal::Status(StatusRefusal::RateLimited) => 11,
            ReadRefusal::Status(StatusRefusal::Challenge) => 12,
            ReadRefusal::Status(StatusRefusal::Client) => 13,
            ReadRefusal::Status(StatusRefusal::Server) => 14,
            ReadRefusal::Status(StatusRefusal::Unexpected) => 15,
            ReadRefusal::WrongContentType => 16,
            ReadRefusal::TooLarge(SizeLimit::Headers) => 17,
            ReadRefusal::TooLarge(SizeLimit::Wire) => 18,
            ReadRefusal::Encoding => 19,
            ReadRefusal::Transport => 20,
        }
    }
}

impl std::fmt::Display for ReadRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, formatter)
    }
}

impl std::error::Error for ReadRefusal {}
