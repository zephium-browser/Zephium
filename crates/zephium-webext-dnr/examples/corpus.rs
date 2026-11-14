use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};
use zephium_webext_dnr::{
    compile_regex, evaluate, parse_rules_json, regex_nfa_memory_usage, CompiledRuleset, DomainType,
    Extension, Request, RequestMethod, ResourceType, Rule, RulesetKind, MAX_RULESET_JSON_BYTES,
};

const CORPUS_ENV: &str = "ZEPHIUM_DNR_CORPUS";
const ARTIFACT_ENV: &str = "ZEPHIUM_DNR_ARTIFACT_DIR";
const MAX_FILES: usize = 10_000;
const MAX_DEPTH: usize = 16;
const MAX_SAMPLES: usize = 4_096;
const ROUNDS: usize = 16;
const LOAD_ROUNDS: usize = 33;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::var_os(CORPUS_ENV)
        .ok_or("set ZEPHIUM_DNR_CORPUS to uBOL's chromium/rulesets directory")?;
    let root = fs::canonicalize(directory)?;
    let artifacts = std::env::var_os(ARTIFACT_ENV).map(PathBuf::from);
    if let Some(directory) = &artifacts {
        fs::create_dir_all(directory)?;
    }
    let enabled = enabled_resources(&root)?;
    let mut files = Vec::new();
    collect_json(&root, 0, &mut files)?;
    files.sort();

    let mut default_bytes = Vec::new();
    let mut default_artifacts = Vec::new();
    let mut samples = Vec::new();
    let mut accepted_total = 0;
    let mut rejected_total = 0;
    let mut bytes_total = 0;
    let mut compile_ns_total = 0;
    let mut rulesets_total = 0;
    let mut unsupported_response_header_total = 0;
    let mut accepted_kinds = BTreeMap::<String, usize>::new();
    let mut rejected_kinds = BTreeMap::<String, usize>::new();
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut regex_sources = Vec::new();
    let mut supported_regex_sources = Vec::new();
    let mut nfa_sizes = Vec::new();
    let mut regex_support_reasons = BTreeMap::<String, usize>::new();
    let mut rust_regex_reasons = BTreeMap::<String, usize>::new();
    let mut rust_regex_supported = 0;
    let mut regex_case_sensitive = 0;
    let mut supported_case_sensitive = 0;
    let mut regex_scan_ns = 0_u64;
    let mut max_ruleset_nfa_bytes = 0;

    for path in files {
        let bytes = read_bounded(&path)?;
        let raw: Value = serde_json::from_slice(&bytes)?;
        let Some(raw_rules) = raw.as_array() else {
            continue;
        };
        let known_directory = path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| matches!(name.to_str(), Some("main" | "regex" | "strictblock")));
        if !known_directory
            && !raw_rules
                .first()
                .is_some_and(|rule| rule.get("action").is_some() && rule.get("condition").is_some())
        {
            continue;
        }

        let relative = path
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        let scan_start = Instant::now();
        let mut ruleset_nfa_bytes = 0;
        for rule in raw_rules {
            let Some(pattern) = rule["condition"]["regexFilter"].as_str() else {
                continue;
            };
            let sensitive = rule["condition"]["isUrlFilterCaseSensitive"].as_bool() == Some(true);
            regex_sources.push(pattern.len() as u64);
            regex_case_sensitive += usize::from(sensitive);
            match regex_nfa_memory_usage(pattern, sensitive) {
                Ok(bytes) => {
                    match compile_regex(pattern, sensitive) {
                        Ok(_) => rust_regex_supported += 1,
                        Err(reason) => increment(&mut rust_regex_reasons, &reason),
                    }
                    supported_regex_sources.push(pattern.len() as u64);
                    nfa_sizes.push(bytes as u64);
                    supported_case_sensitive += usize::from(sensitive);
                    ruleset_nfa_bytes += bytes;
                }
                Err(reason) => increment(&mut regex_support_reasons, &reason),
            }
        }
        regex_scan_ns += scan_start.elapsed().as_nanos() as u64;
        max_ruleset_nfa_bytes = max_ruleset_nfa_bytes.max(ruleset_nfa_bytes);
        let start = Instant::now();
        let parsed = parse_rules_json(&bytes, RulesetKind::Static)?;
        let mut rejected_indices = BTreeSet::new();
        let mut file_reasons = BTreeMap::<String, usize>::new();
        let mut file_rejected = BTreeMap::<String, usize>::new();
        for rejection in &parsed.rejected {
            rejected_indices.insert(rejection.index);
            increment(&mut file_reasons, &rejection.reason);
            increment(&mut reasons, &rejection.reason);
            let kind = rule_kind(raw_rules.get(rejection.index));
            increment(&mut file_rejected, kind);
            increment(&mut rejected_kinds, kind);
        }
        let mut file_accepted = BTreeMap::<String, usize>::new();
        for (index, rule) in raw_rules.iter().enumerate() {
            if !rejected_indices.contains(&index) {
                let kind = rule_kind(Some(rule));
                increment(&mut file_accepted, kind);
                increment(&mut accepted_kinds, kind);
            }
        }
        let is_enabled = enabled.contains(&path);
        if is_enabled {
            samples.extend(parsed.rules.iter().filter_map(Sample::from_rule));
        }
        let count = parsed.rules.len();
        let unsupported_response_headers = parsed
            .rules
            .iter()
            .filter(|rule| rule.action.has_unsupported_response_header_modifications())
            .count();
        let index = CompiledRuleset::compile(&relative, RulesetKind::Static, parsed.rules)?;
        let sealed = index.to_bytes();
        let compile_ns = start.elapsed().as_nanos() as u64;
        let load_start = Instant::now();
        let loaded = CompiledRuleset::load(&sealed)?;
        let load_ns = load_start.elapsed().as_nanos() as u64;
        assert_eq!(loaded.rule_count(), index.rule_count());
        drop(loaded);
        accepted_total += count;
        rejected_total += parsed.rejected.len();
        bytes_total += sealed.len();
        compile_ns_total += compile_ns;
        rulesets_total += 1;
        unsupported_response_header_total += unsupported_response_headers;
        println!(
            "{}",
            json!({"type":"ruleset", "path":relative, "default_enabled":is_enabled,
                "source_bytes":bytes.len(), "accepted":count, "rejected":parsed.rejected.len(),
                "accepted_by_action":file_accepted, "rejected_by_action":file_rejected,
                "unsupported_response_header_rules":unsupported_response_headers,
                "rejection_reasons":file_reasons, "compiled_bytes":sealed.len(),
                "regex_rules":index.regex_count(),"regex_fallback_rules":index.regex_fallback_count(),
                "serialized_dfa_bytes":index.serialized_dfa_bytes(),
                "compile_ns":compile_ns, "load_ns":load_ns})
        );
        if is_enabled {
            let artifact = relative.replace('/', "_").replace(".json", ".dnr");
            if let Some(directory) = &artifacts {
                fs::write(directory.join(&artifact), &sealed)?;
            }
            default_artifacts.push(json!({"path":artifact,"ruleset_id":relative,
                "compiled_bytes":sealed.len(),"rules":count,"first_load_ns":load_ns}));
            default_bytes.push(sealed);
        }
    }
    if rulesets_total == 0 || default_bytes.is_empty() {
        return Err("no DNR rulesets or no manifest-enabled static rulesets found".into());
    }
    if let Some(directory) = &artifacts {
        fs::write(
            directory.join("default-indexes.json"),
            serde_json::to_vec_pretty(&default_artifacts)?,
        )?;
        let warmup = parse_rules_json(
            br#"[
              {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"||warmup.invalid^","requestDomains":["warmup.invalid"],"initiatorDomains":["frame.invalid"],"topDomains":["top.invalid"],"resourceTypes":["script"],"requestMethods":["get"],"domainType":"thirdParty"}},
              {"id":2,"action":{"type":"redirect","redirect":{"url":"https://warmup.invalid/path"}},"condition":{"regexFilter":"warmup[0-9]+"}},
              {"id":3,"action":{"type":"modifyHeaders","requestHeaders":[{"header":"x-warmup","operation":"set","value":"1"}]},"condition":{}},
              {"id":4,"action":{"type":"modifyHeaders","responseHeaders":[{"header":"x-warmup","operation":"remove"}]},"condition":{"responseHeaders":[{"header":"x-warmup","values":["*"]}]}}
            ]"#,
            RulesetKind::Static,
        )?;
        if !warmup.rejected.is_empty() {
            return Err(format!(
                "synthetic loader warmup rules rejected: {:?}",
                warmup.rejected
            )
            .into());
        }
        fs::write(
            directory.join("warmup.dnr"),
            CompiledRuleset::compile("synthetic-loader-warmup", RulesetKind::Static, warmup.rules)?
                .to_bytes(),
        )?;
    }
    println!(
        "{}",
        json!({"type":"total", "rulesets":rulesets_total,"accepted":accepted_total,
            "rejected":rejected_total,"accepted_by_action":accepted_kinds,
            "rejected_by_action":rejected_kinds,"rejection_reasons":reasons,
            "unsupported_response_header_rules":unsupported_response_header_total,
            "compiled_bytes":bytes_total,"compile_ns":compile_ns_total})
    );
    println!(
        "{}",
        json!({"type":"regex_support","patterns":regex_sources.len(),
        "supported":nfa_sizes.len(),"rejected":regex_sources.len()-nfa_sizes.len(),
        "rejection_reasons":regex_support_reasons,"case_sensitive":regex_case_sensitive,
        "rust_regex_supported":rust_regex_supported,"rust_regex_rejection_reasons":rust_regex_reasons,
        "supported_case_sensitive":supported_case_sensitive,"total_nfa_bytes":nfa_sizes.iter().sum::<u64>(),
        "max_ruleset_nfa_bytes":max_ruleset_nfa_bytes,
        "source_bytes":optional_distribution(regex_sources),
        "supported_source_bytes":optional_distribution(supported_regex_sources),
        "nfa_bytes":optional_distribution(nfa_sizes),"diagnostic_ns":regex_scan_ns,
        "timing":"production regex_nfa_memory_usage with actual case flags; independent of whole-file quota admission; excluded from compile_ns"})
    );
    if cfg!(debug_assertions) {
        println!("{}", json!({"type":"benchmark","skipped":"use --release"}));
    } else {
        let mut loads = Vec::with_capacity(LOAD_ROUNDS);
        for _ in 0..LOAD_ROUNDS {
            let start = Instant::now();
            let indexes = default_bytes
                .iter()
                .map(|bytes| CompiledRuleset::load(black_box(bytes)))
                .collect::<Result<Vec<_>, _>>()?;
            loads.push(start.elapsed().as_nanos() as u64);
            black_box(indexes);
        }
        println!(
            "{}",
            json!({"type":"default_load","rulesets":default_bytes.len(),
            "compiled_bytes":default_bytes.iter().map(Vec::len).sum::<usize>(),
            "rounds":LOAD_ROUNDS,"aggregate_ns":distribution(loads),
            "timing":"validation of all default blobs; pre-resident Vec byte slices; no source parsing"})
        );
        load_diagnostics(&default_bytes)?;
        let compiled = default_bytes
            .iter()
            .map(|bytes| CompiledRuleset::load(bytes))
            .collect::<Result<Vec<_>, _>>()?;
        benchmark(&compiled, samples, artifacts.as_deref())?;
        profile_load(&default_bytes)?;
    }
    Ok(())
}

fn profile_load(blobs: &[Vec<u8>]) -> Result<(), Box<dyn Error>> {
    const PHASES: [&str; 6] = [
        "checksum",
        "pool",
        "rows",
        "identifiers",
        "slots",
        "programs",
    ];
    let mut timings: [Vec<u64>; 6] = std::array::from_fn(|_| Vec::with_capacity(LOAD_ROUNDS));
    let mut normal_totals = Vec::with_capacity(LOAD_ROUNDS);
    let mut instrumented_totals = Vec::with_capacity(LOAD_ROUNDS);
    for _ in 0..LOAD_ROUNDS {
        let start = Instant::now();
        for bytes in blobs {
            black_box(CompiledRuleset::load(black_box(bytes))?);
        }
        normal_totals.push(start.elapsed().as_nanos() as u64);
        let start = Instant::now();
        let mut totals = [0_u64; 6];
        for bytes in blobs {
            let mut previous = Instant::now();
            black_box(CompiledRuleset::load_profiled(bytes, |phase| {
                let now = Instant::now();
                let index = PHASES
                    .iter()
                    .position(|&name| name == phase)
                    .expect("known load phase");
                totals[index] += (now - previous).as_nanos() as u64;
                previous = now;
            })?);
        }
        for (phase, total) in timings.iter_mut().zip(totals) {
            phase.push(total);
        }
        instrumented_totals.push(start.elapsed().as_nanos() as u64);
    }
    let phases: BTreeMap<_, _> = PHASES.into_iter().zip(timings.map(distribution)).collect();
    println!(
        "{}",
        json!({"type":"default_load_profile","rounds":LOAD_ROUNDS,
        "phases_ns":phases,"normal_total_ns":distribution(normal_totals),
        "instrumented_total_ns":distribution(instrumented_totals),
        "timing":"aggregate phase durations across six indexes; instrumented and normal loads interleaved; views immediately dropped; callback clock overhead included"})
    );
    Ok(())
}

fn load_diagnostics(blobs: &[Vec<u8>]) -> Result<(), Box<dyn Error>> {
    let field = |bytes: &[u8], offset| {
        u32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("compiled header"),
        ) as usize
    };
    let mut checksums = Vec::with_capacity(LOAD_ROUNDS);
    let mut utf8 = Vec::with_capacity(LOAD_ROUNDS);
    for _ in 0..LOAD_ROUNDS {
        let start = Instant::now();
        for bytes in blobs {
            black_box(crc32fast::hash(black_box(&bytes[field(bytes, 20)..])));
        }
        checksums.push(start.elapsed().as_nanos() as u64);
        let start = Instant::now();
        for bytes in blobs {
            black_box(std::str::from_utf8(black_box(&bytes[field(bytes, 44)..]))?);
        }
        utf8.push(start.elapsed().as_nanos() as u64);
    }
    let layout: Vec<_> = blobs
        .iter()
        .map(|bytes| {
            json!({"rules":field(bytes,16),"occupied_buckets":field(bytes,32),
                "bucket_table_bytes":field(bytes,32)*32,"prefix_count":field(bytes,88),
                "prefix_directory_bytes":(field(bytes,88)+1)*4,
                "string_pool_bytes":field(bytes,48),"bytes":bytes.len()})
        })
        .collect();
    println!(
        "{}",
        json!({"type":"default_load_phases","rounds":LOAD_ROUNDS,
            "checksum_ns":distribution(checksums),"pool_utf8_ns":distribution(utf8),
            "layout":layout,"timing":"separate aggregate passes over all six compiled byte slices"})
    );
    Ok(())
}

fn increment(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_owned()).or_default() += 1;
}

fn rule_kind(rule: Option<&Value>) -> &str {
    rule.and_then(|rule| rule.get("action"))
        .and_then(|action| action.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    if fs::metadata(path)?.len() > MAX_RULESET_JSON_BYTES as u64 {
        return Err(format!("corpus JSON exceeds byte limit: {}", path.display()).into());
    }
    let bytes = fs::read(path)?;
    if bytes.len() > MAX_RULESET_JSON_BYTES {
        return Err("corpus JSON grew beyond byte limit".into());
    }
    Ok(bytes)
}

fn collect_json(
    directory: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    if depth > MAX_DEPTH {
        return Err("corpus directory nesting exceeds limit".into());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_json(&entry.path(), depth + 1, files)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "json") {
            if files.len() >= MAX_FILES {
                return Err("corpus file count exceeds limit".into());
            }
            files.push(entry.path());
        }
    }
    Ok(())
}

fn enabled_resources(root: &Path) -> Result<BTreeSet<PathBuf>, Box<dyn Error>> {
    let extension_root = root.parent().ok_or("rulesets directory has no parent")?;
    let manifest: Value =
        serde_json::from_slice(&read_bounded(&extension_root.join("manifest.json"))?)?;
    let resources = manifest["declarative_net_request"]["rule_resources"]
        .as_array()
        .ok_or("manifest lacks declarative_net_request.rule_resources")?;
    let mut enabled = BTreeSet::new();
    for resource in resources {
        if resource["enabled"].as_bool() != Some(true) {
            continue;
        }
        let path = resource["path"]
            .as_str()
            .ok_or("enabled resource lacks path")?;
        let canonical = fs::canonicalize(extension_root.join(path.trim_start_matches('/')))?;
        if !canonical.starts_with(root) {
            return Err("manifest resource escapes corpus directory".into());
        }
        enabled.insert(canonical);
    }
    Ok(enabled)
}

#[derive(serde::Serialize)]
struct Sample {
    url: String,
    host: String,
    initiator: Option<String>,
    resource_type: ResourceType,
    method: RequestMethod,
    domain_type: DomainType,
}

impl Sample {
    fn from_rule(rule: &Rule) -> Option<Self> {
        let condition = &rule.condition;
        let domain = condition
            .request_domains
            .as_ref()
            .and_then(|items| items.first());
        let pattern = condition.url_filter.as_deref().unwrap_or("");
        let expanded = pattern
            .trim_matches('|')
            .replace('^', "/")
            .replace('*', "sample");
        let candidate = if pattern.starts_with("||") {
            format!("https://{expanded}")
        } else if expanded.starts_with("http://") || expanded.starts_with("https://") {
            expanded
        } else {
            format!(
                "https://{}/{expanded}",
                domain.map_or("sample.invalid", String::as_str)
            )
        };
        let url = url::Url::parse(&candidate).ok()?;
        let host = url.host_str()?.to_owned();
        Some(Self {
            url: url.into(),
            host,
            initiator: condition
                .initiator_domains
                .as_ref()
                .or(condition.domains.as_ref())
                .and_then(|items| items.first())
                .cloned(),
            resource_type: condition
                .resource_types
                .as_ref()
                .and_then(|items| items.first())
                .copied()
                .unwrap_or(ResourceType::Script),
            method: condition
                .request_methods
                .as_ref()
                .and_then(|items| items.first())
                .copied()
                .unwrap_or(RequestMethod::Get),
            domain_type: condition.domain_type.unwrap_or(DomainType::ThirdParty),
        })
    }

    fn request(&self) -> Request<'_> {
        let mut request = Request::new(&self.url, &self.host, self.resource_type);
        request.initiator_host = self.initiator.as_deref();
        request.method = self.method;
        request.domain_type = self.domain_type;
        request
    }
}

fn benchmark(
    compiled: &[CompiledRuleset<'_>],
    samples: Vec<Sample>,
    artifacts: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    if samples.is_empty() {
        return Err("could not synthesize rule-derived URL samples".into());
    }
    let stride = samples.len().div_ceil(MAX_SAMPLES);
    let mut samples: Vec<_> = samples
        .into_iter()
        .step_by(stride)
        .take(MAX_SAMPLES)
        .collect();
    let derived_count = samples.len();
    // Every rule-derived sample has an unrelated control, so misses are measured too.
    for index in 0..derived_count {
        samples.push(Sample {
            url: format!("https://control-{index}.invalid/unrelated/{index}"),
            host: format!("control-{index}.invalid"),
            initiator: None,
            resource_type: ResourceType::Script,
            method: RequestMethod::Get,
            domain_type: DomainType::ThirdParty,
        });
    }
    if let Some(directory) = artifacts {
        fs::write(
            directory.join("requests.json"),
            serde_json::to_vec(&samples)?,
        )?;
    }
    let rulesets: Vec<_> = compiled.iter().collect();
    let extensions = [Extension {
        id: "synthetic-corpus",
        install_order: 1,
        rulesets: &rulesets,
    }];
    let matched_samples = samples
        .iter()
        .filter(|sample| {
            !evaluate(&sample.request(), &extensions)
                .matched_rules
                .is_empty()
        })
        .count();
    for _ in 0..2 {
        for sample in &samples {
            black_box(evaluate(
                black_box(&sample.request()),
                black_box(&extensions),
            ));
        }
    }
    let mut elapsed = Vec::with_capacity(samples.len() * ROUNDS);
    let mut observations = Vec::with_capacity(samples.len() * ROUNDS);
    for round in 0..ROUNDS {
        for index in 0..samples.len() {
            let sample_index = (index + round * 17) % samples.len();
            let sample = &samples[sample_index];
            let request = sample.request();
            let start = Instant::now();
            let result = evaluate(black_box(&request), black_box(&extensions));
            let ns = start.elapsed().as_nanos() as u64;
            black_box(&result);
            elapsed.push(ns);
            observations.push((sample_index, ns));
        }
    }
    elapsed.sort_unstable();
    println!(
        "{}",
        json!({"type":"benchmark", "profile":"release",
        "enabled_rulesets":compiled.len(),"rule_derived_urls":derived_count,
        "control_urls":derived_count,"matched_samples":matched_samples,
        "iterations":elapsed.len(),"rounds":ROUNDS,
        "median_ns":elapsed[elapsed.len()/2],"p99_ns":elapsed[(elapsed.len()*99).div_ceil(100)-1],
        "timing":"one evaluate call; Instant overhead included; result destruction excluded",
        "default_enabled_source":"manifest rule_resources enabled=true (static main rulesets)"})
    );
    let candidates: Vec<u64> = samples
        .iter()
        .map(|sample| {
            let request = sample.request();
            compiled
                .iter()
                .map(|index| index.candidates(&request).count() as u64)
                .sum()
        })
        .collect();
    let mut per_sample: Vec<Vec<u64>> = (0..samples.len())
        .map(|_| Vec::with_capacity(ROUNDS))
        .collect();
    let mut derived_ns = Vec::with_capacity(derived_count * ROUNDS);
    let mut control_ns = Vec::with_capacity(derived_count * ROUNDS);
    for (sample_index, ns) in observations {
        per_sample[sample_index].push(ns);
        if sample_index < derived_count {
            derived_ns.push(ns);
        } else {
            control_ns.push(ns);
        }
    }
    if let Some(directory) = artifacts {
        let profiles: Vec<_> = per_sample
            .iter()
            .enumerate()
            .map(|(index, times)| {
                json!({"sample":index,"candidates":candidates[index],
                    "url_bytes":samples[index].url.len(),"rule_derived":index<derived_count,
                    "evaluate_ns":distribution(times.clone())})
            })
            .collect();
        fs::write(
            directory.join("request-timings.json"),
            serde_json::to_vec(&profiles)?,
        )?;
    }
    let mut slowest: Vec<_> = per_sample
        .into_iter()
        .enumerate()
        .map(|(index, mut times)| {
            times.sort_unstable();
            (times[times.len() / 2], index)
        })
        .collect();
    slowest.sort_unstable_by(|left, right| right.cmp(left));
    let slowest: Vec<_> = slowest
        .into_iter()
        .take(5)
        .map(|(median_ns, index)| {
            let request = samples[index].request();
            let matching_rules: Vec<_> = compiled
                .iter()
                .flat_map(|ruleset| {
                    ruleset
                        .candidates(&request)
                        .filter(|rule| rule.matches(&request))
                        .map(move |rule| {
                            let rule = rule.decode();
                            json!({"ruleset":ruleset.ruleset_id(),"id":rule.id,
                                "request_domains":rule.condition.request_domains.as_ref().map_or(0,Vec::len),
                                "initiator_domains":rule.condition.initiator_domains.as_ref().map_or(0,Vec::len),
                                "top_domains":rule.condition.top_domains.as_ref().map_or(0,Vec::len)})
                        })
                })
                .collect();
            json!({"median_ns":median_ns,"url_bytes":samples[index].url.len(),
            "candidates":candidates[index],"rule_derived":index < derived_count,
            "matching_rule_shapes":matching_rules})
        })
        .collect();
    println!(
        "{}",
        json!({"type":"benchmark_diagnostics",
        "candidate_counts":distribution(candidates),
        "url_bytes":distribution(samples.iter().map(|sample| sample.url.len() as u64).collect()),
        "rule_derived_ns":distribution(derived_ns),"control_ns":distribution(control_ns),
        "slowest_samples_by_median":slowest})
    );
    let mut candidate_ns = Vec::with_capacity(samples.len() * ROUNDS);
    let mut condition_ns = Vec::with_capacity(samples.len() * ROUNDS);
    for round in 0..ROUNDS {
        for index in 0..samples.len() {
            let request = samples[(index + round * 17) % samples.len()].request();
            let start = Instant::now();
            let count: usize = compiled
                .iter()
                .map(|ruleset| ruleset.candidates(black_box(&request)).count())
                .sum();
            candidate_ns.push(start.elapsed().as_nanos() as u64);
            black_box(count);
            let start = Instant::now();
            let count: usize = compiled
                .iter()
                .map(|ruleset| {
                    ruleset
                        .candidates(black_box(&request))
                        .filter(|rule| rule.matches(black_box(&request)))
                        .count()
                })
                .sum();
            condition_ns.push(start.elapsed().as_nanos() as u64);
            black_box(count);
        }
    }
    println!(
        "{}",
        json!({"type":"benchmark_phases","profile":"release","rounds":ROUNDS,
            "candidate_enumeration_ns":distribution(candidate_ns),
            "candidate_conditions_ns":distribution(condition_ns),
            "timing":"same request sample and round order as evaluate; prefilter enumeration and enumeration+condition matches; separate pass"})
    );
    Ok(())
}

fn distribution(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    json!({"min":values[0],"median":values[values.len()/2],
        "p99":values[(values.len()*99).div_ceil(100)-1],"max":values[values.len()-1]})
}

fn optional_distribution(values: Vec<u64>) -> Value {
    if values.is_empty() {
        Value::Null
    } else {
        distribution(values)
    }
}
