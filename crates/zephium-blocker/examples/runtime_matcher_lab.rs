//! Measures the shipped seed compiled for the runtime matcher as the app compiles it: the heap the compiled rules
//! keep, compile time, per-request match time for exact and source-independent attribution, and a digest of every
//! decision so two builds can be compared. The badware list is also compiled alone, as the navigation check does.
//!
//! The corpus is either tab-separated `url<TAB>type<TAB>document` lines with DevTools resource types, or the
//! `corpus.jsonl` that scripts/blocker-corpus records (`url`, `type`, `top`) with WebKit resource types.
//!
//! cargo run --release -p zephium-blocker --example runtime_matcher_lab -- <corpus> [passes]
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use zephium_blocker::{
    CompileTarget, Compiler, FilterSource, NetworkAction, NetworkRequest, RequestMethod,
    ResourceType, SourceFormat, SourceId,
};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            LIVE.fetch_add(size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const BADWARE: &str = "ublock-badware";

fn live_mb() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
}

fn resource(kind: &str) -> Option<ResourceType> {
    Some(match kind {
        "Script" | "script" => ResourceType::Script,
        "Stylesheet" | "style-sheet" => ResourceType::Stylesheet,
        "Image" | "image" | "svg-document" => ResourceType::Image,
        "Font" | "font" => ResourceType::Font,
        "Media" | "media" => ResourceType::Media,
        "XHR" | "raw" => ResourceType::XmlHttpRequest,
        "Fetch" | "fetch" => ResourceType::Fetch,
        "Ping" | "ping" => ResourceType::Beacon,
        "Document" | "document" => ResourceType::Subdocument,
        _ => return None,
    })
}

fn corpus(path: &str) -> Vec<(String, String, ResourceType)> {
    let text = std::fs::read_to_string(path).expect("a readable corpus");
    text.lines()
        .filter_map(|line| {
            if line.starts_with('{') {
                let row: serde_json::Value = serde_json::from_str(line).ok()?;
                let url = row["url"].as_str()?.to_owned();
                let document = row["top"].as_str().unwrap_or("").to_owned();
                Some((url, document, resource(row["type"].as_str()?)?))
            } else {
                let mut fields = line.split('\t');
                let url = fields.next()?.to_owned();
                let kind = resource(fields.next()?)?;
                Some((url, fields.next().unwrap_or("").to_owned(), kind))
            }
        })
        .filter(|(url, _, _)| url.starts_with("http"))
        .collect()
}

fn seed_sources() -> Vec<FilterSource> {
    let seed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/blocker-seed/v1");
    let catalog: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(seed.join("catalog.json")).unwrap()).unwrap();
    catalog["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|source| {
            let target = source["target"].as_str().unwrap();
            let gz = std::fs::read(seed.join(format!("{target}.gz"))).unwrap();
            let mut text = String::new();
            flate2::read::GzDecoder::new(&gz[..])
                .read_to_string(&mut text)
                .unwrap();
            let id = SourceId::new(source["id"].as_str().unwrap()).unwrap();
            FilterSource::new(id, SourceFormat::Standard, text)
        })
        .collect()
}

fn percentile(sorted: &[u128], p: usize) -> u128 {
    sorted
        .get(sorted.len().saturating_sub(1) * p / 100)
        .copied()
        .unwrap_or(0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let corpus = corpus(
        &args
            .next()
            .expect("usage: runtime_matcher_lab <corpus> [passes]"),
    );
    let passes: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(40);

    let badware: Vec<_> = seed_sources()
        .into_iter()
        .filter(|source| source.id().as_str() == BADWARE)
        .collect();
    let before = live_mb();
    let navigation = Compiler::default()
        .compile(CompileTarget::Runtime, badware)
        .unwrap();
    println!("badware alone: live_mb {:.1}", live_mb() - before);
    drop(navigation);

    let sources = seed_sources();
    let before = live_mb();
    let started = Instant::now();
    let compiled = Compiler::default()
        .compile(CompileTarget::Runtime, sources)
        .unwrap();
    let compile_ms = started.elapsed().as_secs_f64() * 1000.0;
    let report = compiled.report();
    println!(
        "all lists: live_mb {:.1} compile_ms {compile_ms:.0} candidate_rules {} rejected_rules {} runtime_omitted_rules {}",
        live_mb() - before,
        report.candidate_rules(),
        report.rejected_rules(),
        report.runtime_omitted_rules()
    );
    println!("corpus_requests {} passes {passes}", corpus.len());

    for exact in [true, false] {
        let evaluate = |(url, document, kind): &(String, String, ResourceType)| {
            if exact {
                compiled.evaluate(NetworkRequest::new(
                    url,
                    document,
                    *kind,
                    RequestMethod::Get,
                ))
            } else {
                compiled.evaluate_source_independent(NetworkRequest::source_independent(
                    url,
                    *kind,
                    RequestMethod::Get,
                ))
            }
        };
        // The first pass settles the lazy DFA caches and records the decisions.
        let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
        let mut blocked = 0;
        for request in &corpus {
            let block = matches!(evaluate(request), Ok(d) if d.action() == NetworkAction::Block);
            blocked += usize::from(block);
            digest = (digest ^ u64::from(block)).wrapping_mul(0x0100_0000_01b3);
        }
        let mut samples = Vec::with_capacity(corpus.len() * passes);
        for _ in 0..passes {
            for request in &corpus {
                let started = Instant::now();
                let _ = std::hint::black_box(evaluate(request));
                samples.push(started.elapsed().as_nanos());
            }
        }
        samples.sort_unstable();
        println!(
            "{}: blocked {blocked} decisions_digest {digest:016x} ns p50 {} p95 {} p99 {} max {} live_mb_after_matching {:.1}",
            if exact { "exact" } else { "source_independent" },
            percentile(&samples, 50),
            percentile(&samples, 95),
            percentile(&samples, 99),
            samples.last().copied().unwrap_or(0),
            live_mb() - before
        );
    }
}
