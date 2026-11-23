use std::{sync::Arc, time::Instant};

use zephium_map::session::{Expect, Failure, FetchResult};
use zephium_public_fetch::{
    Expect as PublicExpect, PublicBody, PublicReader, ReadRefusal, ReadRequest, ReadSession,
    StatusRefusal,
};

use crate::io::{FetchRequest, Fetcher, Reading};

/// One per map: reads share connections only within it.
pub struct PublicIo {
    session: Arc<ReadSession>,
}

impl PublicIo {
    pub fn new(reader: &PublicReader) -> Self {
        Self {
            session: Arc::new(reader.session()),
        }
    }
}

impl Fetcher for PublicIo {
    fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
        let session = Arc::clone(&self.session);
        Box::pin(async move {
            let now = Instant::now();
            let read = ReadRequest {
                url: request.url,
                expect: match request.expect {
                    Expect::Html => PublicExpect::Html,
                    Expect::Xml => PublicExpect::Xml,
                    Expect::Text => PublicExpect::Text,
                },
                max_bytes: request.max_bytes as usize,
                deadline: now.checked_add(request.timeout).unwrap_or(now),
            };
            session
                .read(read)
                .await
                .map(|body| fetch_result(body, request.expect))
                .map_err(failure)
        })
    }
}

fn fetch_result(body: PublicBody, expect: Expect) -> FetchResult {
    let sniffed = match expect {
        Expect::Html => "text/html",
        Expect::Xml => "application/xml",
        Expect::Text => "text/plain",
    };
    FetchResult {
        final_url: body.final_url,
        content_type: match body.media_type.as_deref() {
            Some("application/gzip" | "application/x-gzip") if expect == Expect::Xml => {
                sniffed.into()
            }
            _ => body.media_type.unwrap_or_else(|| sniffed.into()),
        },
        charset: body.charset,
        bytes: body.bytes,
        truncated: body.truncated,
    }
}

pub(crate) fn failure(refusal: ReadRefusal) -> Failure {
    use ReadRefusal as R;
    use StatusRefusal as S;
    match refusal {
        R::NotPublicHttps | R::RedirectRefused | R::TooManyRedirects | R::DnsRefused => {
            Failure::External
        }
        R::ProxyUnsupported | R::RouteChanged | R::Transport | R::Status(S::Denied) => {
            Failure::Unavailable
        }
        R::Timeout(_) => Failure::Timeout,
        R::Status(S::Client) => Failure::Client,
        R::Status(S::RateLimited | S::Server) => Failure::Server,
        R::Status(S::Challenge) => Failure::Challenge,
        R::Status(S::Unexpected) | R::Encoding => Failure::Unreadable,
        R::WrongContentType => Failure::ContentType,
        R::TooLarge(_) => Failure::Limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reader_refusal_maps_to_the_bench_decision() {
        use Failure as F;
        let expected = [
            F::External,
            F::External,
            F::External,
            F::External,
            F::Unavailable,
            F::Unavailable,
            F::Timeout,
            F::Timeout,
            F::Timeout,
            F::Timeout,
            F::Unavailable,
            F::Server,
            F::Challenge,
            F::Client,
            F::Server,
            F::Unreadable,
            F::ContentType,
            F::Limit,
            F::Limit,
            F::Unreadable,
            F::Unavailable,
        ];
        assert_eq!(ReadRefusal::ALL.map(failure), expected);
    }

    #[test]
    fn sniffed_and_compressed_sitemaps_keep_the_expected_media_type() {
        let body = |media: Option<&str>| PublicBody {
            final_url: url::Url::parse("https://acme.com/sitemap.xml").unwrap(),
            hops: 0,
            status: zephium_public_fetch::StatusClass::Ok,
            media_type: media.map(str::to_owned),
            charset: None,
            bytes: Vec::new(),
            truncated: false,
            wire_bytes: 0,
            timings: Default::default(),
        };
        assert_eq!(
            fetch_result(body(None), Expect::Html).content_type,
            "text/html"
        );
        assert_eq!(
            fetch_result(body(Some("application/gzip")), Expect::Xml).content_type,
            "application/xml"
        );
        assert_eq!(
            fetch_result(body(Some("application/gzip")), Expect::Html).content_type,
            "application/gzip"
        );
    }
}
