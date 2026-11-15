use std::hint::black_box;
use std::time::Instant;
use zephium_webext_matcher::injection::{MatchOptions, MatchSet};

#[test]
#[ignore = "release-only benchmark with a verified package's pattern list"]
fn package_pattern_cost() {
    if cfg!(debug_assertions) {
        panic!("capacity measurements require release mode");
    }
    let path = std::env::var_os("ZEPHIUM_MATCH_PATTERNS_FILE").unwrap();
    let patterns = std::fs::read_to_string(path).unwrap();
    let patterns: Vec<_> = patterns.lines().collect();
    assert_eq!(patterns.len(), 384);
    for count in [128, 384, 768] {
        let input: Vec<_> = patterns.iter().cycle().take(count).copied().collect();
        let started = Instant::now();
        let set = MatchSet::parse(input, Vec::<&str>::new(), MatchOptions::default()).unwrap();
        let compile_ns = started.elapsed().as_nanos();
        for (label, url) in [
            ("localhost-miss", "http://127.0.0.1/breadth"),
            ("https-miss", "https://example.invalid/path?x=1"),
            ("google", "https://www.google.com/recaptcha/api2/anchor"),
        ] {
            let url = url::Url::parse(url).unwrap();
            let mut samples = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                for _ in 0..20_000 {
                    black_box(set.matches_url(black_box(&url)));
                }
                samples.push(started.elapsed().as_nanos() as f64 / 20_000.0);
            }
            println!("MATCH_CAPACITY count={count} case={label} compile_ns={compile_ns} samples_ns={samples:?} retained_bytes={}", set.retained_budget_bytes());
        }
    }
}
