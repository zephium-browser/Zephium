use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};
use zephium_webext_dnr::{
    evaluate, parse_rules_json, CompiledRuleset, DomainType, Extension, Request, RequestMethod,
    ResourceType, Rule, RulesetKind, MAX_RULESET_JSON_BYTES,
};

const CORPUS_ENV: &str = "ZEPHIUM_DNR_CORPUS";
const MAX_FILES: usize = 10_000;
const MAX_DEPTH: usize = 16;
const MAX_SAMPLES: usize = 4_096;
const ROUNDS: usize = 16;

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::var_os(CORPUS_ENV)
        .ok_or("set ZEPHIUM_DNR_CORPUS to uBOL's chromium/rulesets directory")?;
    let root = fs::canonicalize(directory)?;
    let enabled = enabled_resources(&root)?;
    let mut files = Vec::new();
    collect_json(&root, 0, &mut files)?;
    files.sort();

    let mut compiled = Vec::new();
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
        assert_eq!(loaded.rules().len(), index.rules().len());
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
                "compile_ns":compile_ns, "load_ns":load_ns})
        );
        if is_enabled {
            compiled.push(loaded);
        }
    }
    if rulesets_total == 0 || compiled.is_empty() {
        return Err("no DNR rulesets or no manifest-enabled static rulesets found".into());
    }
    println!(
        "{}",
        json!({"type":"total", "rulesets":rulesets_total,"accepted":accepted_total,
            "rejected":rejected_total,"accepted_by_action":accepted_kinds,
            "rejected_by_action":rejected_kinds,"rejection_reasons":reasons,
            "unsupported_response_header_rules":unsupported_response_header_total,
            "compiled_bytes":bytes_total,"compile_ns":compile_ns_total})
    );
    if cfg!(debug_assertions) {
        println!("{}", json!({"type":"benchmark","skipped":"use --release"}));
    } else {
        benchmark(&compiled, samples)?;
    }
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

fn benchmark(compiled: &[CompiledRuleset], samples: Vec<Sample>) -> Result<(), Box<dyn Error>> {
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
            json!({"median_ns":median_ns,"url_bytes":samples[index].url.len(),
            "candidates":candidates[index],"rule_derived":index < derived_count})
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
    Ok(())
}

fn distribution(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    json!({"min":values[0],"median":values[values.len()/2],
        "p99":values[(values.len()*99).div_ceil(100)-1],"max":values[values.len()-1]})
}
