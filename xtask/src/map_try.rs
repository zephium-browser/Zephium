mod printer;
#[cfg(test)]
mod tests;

use std::{
    fmt::{self, Write as _},
    future::Future,
    io::{self, Write},
    sync::Arc,
    time::Instant,
};

use tokio::{runtime::Handle, sync::watch};
use url::{Host, Url};
use zephium_core::search::SearchEngine;
use zephium_map::{
    read_query,
    session::{Failure, FetchResult},
    QueryKind,
};
use zephium_map_driver::{
    wire::{Change, Outcome, StartRefusal},
    Counts, Driver, FetchRequest, Fetcher, Io, MapRequest, MemoryMemo, PublicIo, Reading,
    ResultsPage, ResultsSource, SystemClock,
};
use zephium_public_fetch::{
    guard::{public_target, MAX_HOST_BYTES},
    PublicReader, PublicRoute, ReaderLimits, RouteSource,
};

use self::printer::{intro, refusal, Printer};

const ENGINE: SearchEngine = SearchEngine::DuckDuckGo;
const MAX_SITES: usize = 2;
const MAX_DNS_LABEL_BYTES: usize = 63;

#[derive(Debug, PartialEq, Eq)]
enum Error {
    MissingQuery,
    MissingSite,
    BadHost,
    TooManySites,
    UnexpectedOption,
    PlaceArity,
    CompareArity,
    Refused(StartRefusal),
    Proxy,
    Runtime,
    Interrupt,
    Stream,
    Output,
}

impl Error {
    fn usage(&self) -> bool {
        matches!(
            self,
            Self::MissingQuery
                | Self::MissingSite
                | Self::BadHost
                | Self::TooManySites
                | Self::UnexpectedOption
                | Self::PlaceArity
                | Self::CompareArity
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingQuery => {
                "Usage: cargo xtask map <query> --site <host> [--site <host>] [--private]"
            }
            Self::MissingSite => "Usage error: specify a host with --site",
            Self::BadHost => "Usage error: --site needs a bare public hostname",
            Self::TooManySites => "Usage error: specify at most two sites",
            Self::UnexpectedOption => {
                "Usage error: only --site and one --private flag are supported"
            }
            Self::PlaceArity => "Usage error: a place query needs one --site",
            Self::CompareArity => "Usage error: a comparison needs two --site flags",
            Self::Refused(reason) => return f.write_str(refusal(*reason)),
            Self::Proxy => "The reader refuses to go around a proxy. A direct route is required.",
            Self::Runtime => "Could not start the map runtime.",
            Self::Interrupt => "Could not listen for Ctrl-C.",
            Self::Stream => "The map stopped without an outcome.",
            Self::Output => "Could not write the map to the terminal.",
        })
    }
}

struct Options<'a> {
    query: &'a str,
    sites: Vec<Url>,
    private: bool,
}

impl<'a> Options<'a> {
    fn parse(arguments: &'a [String]) -> Result<Self, Error> {
        let (query, rest) = arguments.split_first().ok_or(Error::MissingQuery)?;
        if query.starts_with("--") {
            return Err(Error::MissingQuery);
        }

        let mut options = Self {
            query,
            sites: Vec::new(),
            private: false,
        };
        let mut args = rest.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--site" => {
                    if options.sites.len() == MAX_SITES {
                        return Err(Error::TooManySites);
                    }
                    let raw = args
                        .next()
                        .filter(|raw| !raw.starts_with("--"))
                        .ok_or(Error::MissingSite)?;
                    options.sites.push(site_url(raw).ok_or(Error::BadHost)?);
                }
                "--private" if !options.private => options.private = true,
                _ => return Err(Error::UnexpectedOption),
            }
        }
        if options.sites.is_empty() {
            return Err(Error::MissingSite);
        }
        Ok(options)
    }

    fn prepare(self) -> Result<(MapRequest, StandIn), Error> {
        let query = read_query(self.query).map_err(|_| Error::Refused(StartRefusal::Invalid))?;
        let titles = match query.kind() {
            QueryKind::Place { entity, .. } => {
                if self.sites.len() != 1 {
                    return Err(Error::PlaceArity);
                }
                vec![if entity.is_empty() {
                    self.query.to_owned()
                } else {
                    original_terms(self.query, entity)
                }]
            }
            QueryKind::Compare { left, right, .. } => {
                if self.sites.len() != MAX_SITES {
                    return Err(Error::CompareArity);
                }
                vec![
                    original_terms(self.query, left),
                    original_terms(self.query, right),
                ]
            }
            _ => return Err(Error::Refused(StartRefusal::NotAMap)),
        };
        let stand_in = StandIn::new(self.sites.iter().zip(titles.iter().map(String::as_str)));
        let request = MapRequest {
            query: self.query.to_owned(),
            engine: ENGINE,
            session_id: 1,
            profile: "terminal".into(),
            private: self.private,
        };
        Ok((request, stand_in))
    }
}

fn site_url(raw: &str) -> Option<Url> {
    let bare = raw.strip_suffix('.').unwrap_or(raw);
    if bare.is_empty()
        || bare.len() > MAX_HOST_BYTES
        || bare.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || (c.is_ascii() && !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.')))
        })
    {
        return None;
    }
    let url = Url::parse(&format!("https://{bare}/")).ok()?;
    let Host::Domain(domain) = url.host()? else {
        return None;
    };
    (domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= MAX_DNS_LABEL_BYTES
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) && public_target(&url))
    .then_some(url)
}

fn original_terms(text: &str, terms: &[String]) -> String {
    let mut words = text.split_whitespace();
    terms
        .iter()
        .filter_map(|term| words.find(|word| word.to_lowercase() == *term))
        .collect::<Vec<_>>()
        .join(" ")
}

struct StandIn(String);

impl StandIn {
    fn new<'a>(results: impl IntoIterator<Item = (&'a Url, &'a str)>) -> Self {
        let mut html = String::from("<main>");
        for (url, title) in results {
            let _ = write!(
                html,
                "<article><h2><a href=\"{}\">{}</a></h2></article>",
                escape_html(url.as_str()),
                escape_html(title)
            );
        }
        html.push_str("</main>");
        Self(html)
    }
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

impl ResultsSource for StandIn {
    fn load(&self, url: Url) -> Reading<Result<ResultsPage, Failure>> {
        let html = self.0.clone();
        Box::pin(async move {
            Ok(ResultsPage {
                html,
                url,
                truncated: false,
            })
        })
    }
}

struct DirectIo {
    routes: RouteSource,
    public: PublicIo,
}

impl Fetcher for DirectIo {
    fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
        let routes = self.routes.clone();
        let fetch = self.public.fetch(request);
        Box::pin(async move {
            // On the current-thread runtime, route publication cannot interleave
            // between this check and the reader taking its route snapshot.
            if routes.route() != PublicRoute::Direct {
                return Err(Failure::Unavailable);
            }
            fetch.await
        })
    }
}

fn on_direct_route<T>(
    route: PublicRoute,
    start: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    if route != PublicRoute::Direct {
        return Err(Error::Proxy);
    }
    start()
}

fn print_lines(
    output: &mut impl Write,
    lines: impl IntoIterator<Item = String>,
) -> Result<(), Error> {
    for line in lines {
        writeln!(output, "{line}").map_err(|_| Error::Output)?;
    }
    output.flush().map_err(|_| Error::Output)
}

async fn live(
    request: MapRequest,
    results: StandIn,
    output: &mut impl Write,
) -> Result<Outcome, Error> {
    print_lines(output, [intro(&request.query)])?;
    let runtime = Handle::current();
    let routes = RouteSource::new(&runtime);
    routes.set_app_proxy(None);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let started = Instant::now();
    tokio::select! {
        biased;
        signal = &mut interrupt => {
            signal.map_err(|_| Error::Interrupt)?;
            let end = Change::End { seq: 0, outcome: Outcome::Cancelled, elapsed_ms: elapsed_ms(started) };
            print_lines(output, Printer::default().lines(&end, started.elapsed(), Counts::default()))?;
            return Ok(Outcome::Cancelled);
        }
        () = routes.settled() => {}
    }

    let changed = routes.changed();
    let (driver, changes) = on_direct_route(routes.route(), || {
        let reader = PublicReader::new(ReaderLimits::default(), routes.clone());
        let io = Io {
            results: Arc::new(results),
            fetch: Arc::new(DirectIo {
                routes,
                public: PublicIo::new(&reader),
            }),
            memo: Arc::new(MemoryMemo::new()),
            clock: Arc::new(SystemClock::new()),
        };
        Driver::start(request, io, &runtime).map_err(Error::Refused)
    })?;
    stream(&driver, changes, changed, interrupt, output).await
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

async fn stream(
    driver: &Driver,
    mut changes: tokio::sync::mpsc::Receiver<Change>,
    mut route: watch::Receiver<PublicRoute>,
    mut interrupt: impl Future<Output = io::Result<()>> + Unpin,
    output: &mut impl Write,
) -> Result<Outcome, Error> {
    let started = Instant::now();
    let mut printer = Printer::default();
    let mut cancelling = false;
    let mut watching = true;
    let mut interrupted_by = None;
    loop {
        let change = tokio::select! {
            biased;
            signal = &mut interrupt, if !cancelling => {
                interrupted_by = signal.err().map(|_| Error::Interrupt);
                driver.cancel();
                cancelling = true;
                continue;
            }
            left = route.wait_for(|class| *class != PublicRoute::Direct), if !cancelling && watching => {
                if left.is_err() {
                    watching = false;
                    continue;
                }
                interrupted_by = Some(Error::Proxy);
                driver.cancel();
                cancelling = true;
                continue;
            }
            change = changes.recv() => change.ok_or(Error::Stream)?,
        };
        if let Change::End { outcome, .. } = change {
            print_lines(
                output,
                printer.lines(&change, started.elapsed(), driver.counts()),
            )?;
            return interrupted_by.map_or(Ok(outcome), Err);
        }
        if !cancelling {
            let lines = printer.lines(&change, started.elapsed(), Counts::default());
            if !lines.is_empty() {
                print_lines(output, lines)?;
            }
        }
    }
}

fn execute(arguments: &[String]) -> Result<Outcome, Error> {
    let (request, results) = Options::parse(arguments)?.prepare()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::Runtime)?;
    let result = runtime.block_on(live(request, results, &mut io::stdout().lock()));
    runtime.shutdown_background();
    result
}

pub(crate) fn run(arguments: &[String]) -> i32 {
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(io::stderr(), "Unexpected map runtime failure.");
    }));
    match execute(arguments) {
        Ok(Outcome::Done) => 0,
        Ok(Outcome::Cancelled) => 130,
        Ok(Outcome::Failed { .. }) => 1,
        Err(error) => {
            let _ = writeln!(io::stderr(), "{error}");
            if error.usage() {
                2
            } else {
                1
            }
        }
    }
}
