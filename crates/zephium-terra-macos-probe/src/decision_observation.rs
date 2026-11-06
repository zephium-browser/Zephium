use std::io::Write as _;
use zephium_agentic::*;
use zephium_engine::DecisionObservationSite;

use super::ProbeFailure;

pub(super) fn run(site: &std::ffi::OsStr) -> Result<(), ProbeFailure> {
    let (site, name) = match site.to_str() {
        Some("airbnb") => (DecisionObservationSite::Airbnb, "airbnb"),
        Some("yc") => (DecisionObservationSite::Yc, "yc"),
        Some("government") => (DecisionObservationSite::Government, "government"),
        Some("demo-store") => (DecisionObservationSite::DemoStore, "demo-store"),
        Some("book-store") => (DecisionObservationSite::BookStore, "book-store"),
        Some("book-catalog") => (DecisionObservationSite::BookCatalog, "book-catalog"),
        Some("test-store") => (DecisionObservationSite::TestStore, "test-store"),
        Some("airbnb-listing") => (DecisionObservationSite::AirbnbListing, "airbnb-listing"),
        Some("lego-theme") => (DecisionObservationSite::LegoTheme, "lego-theme"),
        Some("consent") => (DecisionObservationSite::Consent, "consent"),
        Some("interstitial") => (DecisionObservationSite::Interstitial, "interstitial"),
        Some("documentation") => (DecisionObservationSite::Documentation, "documentation"),
        Some("airbnb-stays") => (DecisionObservationSite::AirbnbStays, "airbnb-stays"),
        Some("airbnb-monthly") => (DecisionObservationSite::AirbnbMonthly, "airbnb-monthly"),
        _ => return Err(ProbeFailure::Authority),
    };
    zephium_engine::run_macos_decision_observation_probe(site, move |observation| {
        let budget = SemanticModelEncodingBudget::INITIAL_PROVIDER_EXACT_CONSERVATIVE;
        let _ = writeln!(std::io::stderr(), "decision_observation captured {}", PageFacts::of(&observation));
        let observation =
            fit_semantic_observation_for_model(observation, budget).map_err(|_| "projection")?;
        if observation.frames().iter().flat_map(|frame| frame.nodes())
            .any(|node| node.sensitivity() != SemanticSensitivity::Public) {
            return Err("non_public_observation");
        }
        let payload = encode_semantic_observation(&observation, budget)
            .and_then(|encoded| {
                encoded.admit_conservative_utf8(
                    &SemanticTokenizerRevision::try_new("public-eval-utf8-upper-bound-v1".into())
                        .map_err(|_| SemanticModelEncodingError::Invariant)?,
                )
            })
            .map_err(|_| "encoding")?;
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/work-runtime-proof/decision-observations");
        std::fs::create_dir_all(&directory).map_err(|_| "output_directory")?;
        std::fs::write(directory.join(format!("{name}.zsem")), payload.as_str())
            .map_err(|_| "output")?;
        let objective = AgentProviderObjective::try_admit_conservative_utf8(match site {
            DecisionObservationSite::Yc => "Read the duration of the YC program and explain how office hours with YC partners work.",
            DecisionObservationSite::Airbnb
            | DecisionObservationSite::AirbnbStays
            | DecisionObservationSite::AirbnbMonthly => "Find three places to stay in San Francisco for a solo founder during a YC batch.",
            DecisionObservationSite::Government => "Read the official travel guidance available on this government page.",
            DecisionObservationSite::DemoStore
            | DecisionObservationSite::BookStore
            | DecisionObservationSite::TestStore => "Read this product page and report the product name, its displayed price and the product picture.",
            DecisionObservationSite::AirbnbListing => "Read this listing and report its name, displayed nightly price, displayed monthly total, stay dates or minimum stay, and picture.",
            DecisionObservationSite::LegoTheme => "Collect three LEGO Architecture sets from this catalog with their displayed prices, product links and pictures.",
            DecisionObservationSite::BookCatalog => "Collect three books from this catalog with their displayed prices, availability, product links and pictures.",
            DecisionObservationSite::Consent => "List the product categories this store's home page offers.",
            DecisionObservationSite::Interstitial => "List the clothing categories this store's home page offers.",
            DecisionObservationSite::Documentation => "List every situation in which WAL mode does not work or has drawbacks.",
        }.into(), &SemanticTokenizerRevision::try_new("public-eval-utf8-upper-bound-v1".into()).map_err(|_| "tokenizer")?)
            .map_err(|_| "objective")?;
        let mut entries = Vec::new();
        for frame in observation.frames() {
            // As the Work reading policy does: while a dialog is shown, only
            // its own controls are offered, and a dialog's buttons are.
            let dialog = frame.nodes().iter().position(|node| node.role() == SemanticRole::Dialog);
            let inside = |node: &SemanticNode| {
                let mut parent = node.parent();
                while let Some(index) = parent {
                    if Some(usize::from(index)) == dialog { return true; }
                    parent = frame.nodes().get(usize::from(index)).and_then(SemanticNode::parent);
                }
                false
            };
            for node in frame.nodes() {
                if dialog.is_some() && !inside(node) { continue; }
                let mut ops = Vec::new();
                if node.operations().contains(SemanticOperationClass::Scroll) { ops.push(SemanticOperationClass::Scroll); }
                if node.role() == SemanticRole::Link && node.operations().contains(SemanticOperationClass::Click)
                    && node.link_destination().is_some_and(|target| target.as_url().host_str() == frame.frame().origin().as_url().host_str()) {
                    ops.push(SemanticOperationClass::Click);
                }
                if dialog.is_some() && node.role() == SemanticRole::Button && node.operations().contains(SemanticOperationClass::Click) {
                    ops.push(SemanticOperationClass::Click);
                }
                if !ops.is_empty() { entries.push((node.reference(), SemanticOperations::try_new(&ops).map_err(|_| "operations")?)); }
            }
        }
        let authority = AgentProviderActionAuthority::try_new(&observation, &entries).ok_or("authority")?;
        let account = AgentContextAccountBinding::new(AgentAccountAttestationId::generate(), observation.request().context(),
            AgentAccountScope::Anonymous, AgentPolicyInstant::from_millis(1));
        if matches!(
            site,
            DecisionObservationSite::DemoStore
                | DecisionObservationSite::BookStore
                | DecisionObservationSite::TestStore
        ) {
            let fields = vec![
                SemanticExtractionFieldSchema::try_text("product_name".into(), true, 512).and_then(SemanticExtractionFieldSchema::with_verbatim_text).map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_text("displayed_price".into(), true, 64).and_then(SemanticExtractionFieldSchema::with_verbatim_text).map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_image_url("product_picture".into(), false, 2048).map_err(|_| "read_schema")?,
            ];
            let schema = SemanticExtractionSchema::try_new(SemanticExtractionSchemaId::new(1).ok_or("read_schema")?, fields).map_err(|_| "read_schema")?;
            let (read, _) = DecisionObservation::try_for_read(&observation, &objective, &authority, account, Some(&schema))
                .and_then(DecisionObservation::into_anonymous_eval_requests)
                .map_err(|error| match error {
                    DecisionProjectionError::Capacity => "read_projection_capacity",
                    DecisionProjectionError::Authority => "read_projection_authority",
                })?;
            std::fs::write(directory.join(format!("{name}-read.json")), read.encode().map_err(|_| "decision_encoding")?).map_err(|_| "decision_output")?;
        }
        if matches!(site, DecisionObservationSite::AirbnbListing) {
            let verbatim = |name: &str, required| SemanticExtractionFieldSchema::try_text(name.into(), required, 512).and_then(SemanticExtractionFieldSchema::with_verbatim_text);
            let fields = vec![
                verbatim("listing_name", true).map_err(|_| "read_schema")?,
                verbatim("nightly_price", false).map_err(|_| "read_schema")?,
                verbatim("monthly_total_displayed", false).map_err(|_| "read_schema")?,
                verbatim("stay_dates_or_minimum", false).map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_image_url("listing_picture".into(), false, 2048).map_err(|_| "read_schema")?,
            ];
            let schema = SemanticExtractionSchema::try_new(SemanticExtractionSchemaId::new(1).ok_or("read_schema")?, fields).map_err(|_| "read_schema")?;
            let (read, _) = DecisionObservation::try_for_read(&observation, &objective, &authority, account, Some(&schema))
                .and_then(DecisionObservation::into_anonymous_eval_requests)
                .map_err(|error| match error {
                    DecisionProjectionError::Capacity => "read_projection_capacity",
                    DecisionProjectionError::Authority => "read_projection_authority",
                })?;
            std::fs::write(directory.join(format!("{name}-read.json")), read.encode().map_err(|_| "decision_encoding")?).map_err(|_| "decision_output")?;
        }
        if matches!(site, DecisionObservationSite::BookCatalog) {
            let verbatim = |name: &str| SemanticExtractionFieldSchema::try_text(name.into(), false, 1024).and_then(SemanticExtractionFieldSchema::with_verbatim_text);
            let columns = vec![
                SemanticExtractionFieldSchema::try_text("name".into(), true, 512).map_err(|_| "read_schema")?,
                verbatim("displayed_price").map_err(|_| "read_schema")?,
                verbatim("availability").map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_url("product_url".into(), false, 2048).map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_image_url("image".into(), false, 2048).map_err(|_| "read_schema")?,
            ];
            let rows = SemanticExtractionFieldSchema::try_rows("output_0".into(), true, columns, 3).map_err(|_| "read_schema")?;
            let schema = SemanticExtractionSchema::try_new(SemanticExtractionSchemaId::new(1).ok_or("read_schema")?, vec![rows]).map_err(|_| "read_schema")?;
            let (read, _) = DecisionObservation::try_for_read(&observation, &objective, &authority, account, Some(&schema))
                .and_then(DecisionObservation::into_anonymous_eval_requests)
                .map_err(|_| "read_projection")?;
            std::fs::write(directory.join(format!("{name}-read.json")), read.encode().map_err(|_| "decision_encoding")?).map_err(|_| "decision_output")?;
        }
        if matches!(site, DecisionObservationSite::Yc) {
            let fields = vec![
                SemanticExtractionFieldSchema::try_text("program_duration".into(), true, 1024).and_then(SemanticExtractionFieldSchema::with_verbatim_text).map_err(|_| "read_schema")?,
                SemanticExtractionFieldSchema::try_text("office_hours".into(), true, 1024).map_err(|_| "read_schema")?,
            ];
            let schema = SemanticExtractionSchema::try_new(SemanticExtractionSchemaId::new(1).ok_or("read_schema")?, fields).map_err(|_| "read_schema")?;
            let (read, _) = DecisionObservation::try_for_read(&observation, &objective, &authority, account, Some(&schema))
                .and_then(DecisionObservation::into_anonymous_eval_requests).map_err(|_| "read_projection")?;
            std::fs::write(directory.join("yc-read.json"), read.encode().map_err(|_| "decision_encoding")?).map_err(|_| "decision_output")?;
        }
        let (decision, json_comparison) = DecisionObservation::try_new(&observation, &objective, &authority, account)
            .and_then(DecisionObservation::into_anonymous_eval_requests).map_err(|_| "decision_projection")?;
        std::fs::write(directory.join(format!("{name}.json")), decision.encode().map_err(|_| "decision_encoding")?)
            .map_err(|_| "decision_output")?;
        if let Some(comparison) = json_comparison {
            std::fs::write(directory.join(format!("{name}-json.json")), comparison.encode().map_err(|_| "decision_encoding")?)
                .map_err(|_| "decision_output")?;
        }
        let _ = writeln!(std::io::stderr(), "decision_observation fitted {}", PageFacts::of(&observation));
        let _ = writeln!(
            std::io::stderr(),
            "decision_observation phase=captured site={site:?} frames={} nodes={} state_bytes={}",
            observation.frames().len(),
            observation
                .frames()
                .iter()
                .map(|frame| frame.nodes().len())
                .sum::<usize>(),
            payload.as_str().len()
        );
        Ok(())
    })
    .map_err(|reason| {
        let _ = writeln!(
            std::io::stderr(),
            "decision_observation phase=failed reason={reason}"
        );
        ProbeFailure::Verification
    })
}

/// The release seed with every list the app ships, admitted under limits
/// that take them all.
fn release_seed() -> Result<zephium_blocker_service::ReleaseCatalogSeed, ProbeFailure> {
    use zephium_blocker_service::{
        EmbeddedReleaseAsset, LicensePolicy, ReleaseCatalogSeed, UpdateLimits,
    };
    let assets = vec![
        EmbeddedReleaseAsset::new(
            "easylist.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/easylist.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "easyprivacy.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/easyprivacy.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-filters.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-filters.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-quick-fixes.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-quick-fixes.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-link-shorteners.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-link-shorteners.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-privacy.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-privacy.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-resource-abuse.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-resource-abuse.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-unbreak.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-unbreak.txt.gz"
            )),
        ),
        EmbeddedReleaseAsset::new(
            "ublock-badware.txt",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/blocker-seed/v1/ublock-badware.txt.gz"
            )),
        ),
    ];
    let limits = UpdateLimits {
        max_manifest_bytes: 16 * 1024,
        max_sources: assets.len(),
        max_source_bytes: 4 * 1024 * 1024,
        max_total_source_bytes: 8 * 1024 * 1024,
        ..UpdateLimits::default()
    };
    let licenses = LicensePolicy::new(["CC-BY-SA-3.0", "GPL-3.0-only"])
        .map_err(|_| ProbeFailure::Authority)?;
    ReleaseCatalogSeed::from_embedded_gzip(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/blocker-seed/v1/catalog.json"
        )),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/blocker-seed/v1/release-seed.json"
        )),
        assets,
        limits,
        licenses,
    )
    .map_err(|_| ProbeFailure::Authority)
}

/// Compiles the app's release lists, every one the app ships, exactly as a
/// protected profile would, so one recording can be compared with none.
pub(super) fn use_release_lists() -> Result<(), ProbeFailure> {
    use zephium_blocker_service::ManagedBlocker;
    use zephium_core::ports::blocker::{BlockerCompileOutcome, BlockerCompiler as _};
    let seed = release_seed()?;
    let cache = tempfile::tempdir().map_err(|_| ProbeFailure::Runtime)?;
    let blocker = ManagedBlocker::with_release_seed(
        seed,
        zephium_blocker::CompiledArtifactCacheConfig::new(cache.path().join("compiled"))
            .map_err(|_| ProbeFailure::Authority)?,
    )
    .map_err(|_| ProbeFailure::Runtime)?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let _ = blocker.compile(
        zephium_core::ids::ProfileId::generate(),
        zephium_core::blocker::ContentPolicyGeneration::new(1).ok_or(ProbeFailure::Authority)?,
        zephium_core::blocker::BlockerConfig { enabled: true },
        Box::new(move |outcome| {
            let _ = tx.send(outcome);
        }),
    );
    let rules = match rx.recv_timeout(std::time::Duration::from_secs(120)) {
        Ok(BlockerCompileOutcome::Compiled(rules)) => rules,
        Ok(BlockerCompileOutcome::Failed(failure)) => {
            let _ = writeln!(
                std::io::stderr(),
                "decision_observation release_lists=failed reason={failure:?}"
            );
            return Err(ProbeFailure::Runtime);
        }
        Err(_) => return Err(ProbeFailure::Runtime),
    };
    let _ = writeln!(
        std::io::stderr(),
        "decision_observation release_lists=compiled"
    );
    zephium_engine::use_macos_decision_observation_content_rules(rules)
        .then_some(())
        .ok_or(ProbeFailure::Runtime)
}

/// Closed facts about what a page served: counts and marker presence only,
/// never its text.
struct PageFacts {
    nodes: usize,
    text_bytes: usize,
    headings: usize,
    links: usize,
    room_links: usize,
    images: usize,
    buttons: usize,
    dialogs: usize,
    error_marker: bool,
    challenge_marker: bool,
}

/// Closed wording of an error or maintenance page.
const ERROR_MARKERS: &[&str] = &[
    "something went wrong",
    "maintenance",
    "timed out",
    "page not found",
    "try again later",
    "we're having trouble",
    "unexpected error",
];
/// Closed wording of bot detection or a human check.
const CHALLENGE_MARKERS: &[&str] = &[
    "captcha",
    "verify you are human",
    "are you a robot",
    "press and hold",
    "press & hold",
    "unusual traffic",
    "access denied",
    "security check",
    "checking your browser",
    "request blocked",
];

impl PageFacts {
    fn of(observation: &SemanticObservation) -> Self {
        let nodes: Vec<&SemanticNode> = observation
            .frames()
            .iter()
            .flat_map(|frame| frame.nodes())
            .collect();
        let texts = || {
            nodes.iter().flat_map(|node| {
                [node.name(), node.text()]
                    .into_iter()
                    .flatten()
                    .map(|text| text.as_str().to_lowercase())
            })
        };
        let marked =
            |markers: &[&str]| texts().any(|text| markers.iter().any(|m| text.contains(m)));
        let role = |role: SemanticRole| nodes.iter().filter(|node| node.role() == role).count();
        Self {
            nodes: nodes.len(),
            text_bytes: nodes
                .iter()
                .flat_map(|node| [node.name(), node.text()].into_iter().flatten())
                .map(SemanticText::len)
                .sum(),
            headings: nodes
                .iter()
                .filter(|node| node.heading_level().is_some())
                .count(),
            links: role(SemanticRole::Link),
            room_links: nodes
                .iter()
                .filter_map(|node| node.link_destination())
                .filter(|target| target.as_url().path().starts_with("/rooms/"))
                .count(),
            images: nodes
                .iter()
                .filter(|node| node.image_source().is_some())
                .count(),
            buttons: role(SemanticRole::Button),
            dialogs: role(SemanticRole::Dialog),
            error_marker: marked(ERROR_MARKERS),
            challenge_marker: marked(CHALLENGE_MARKERS),
        }
    }
}

impl std::fmt::Display for PageFacts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "page_facts nodes={} text_bytes={} headings={} links={} room_links={} images={} buttons={} dialogs={} error_marker={} challenge_marker={}",
            self.nodes,
            self.text_bytes,
            self.headings,
            self.links,
            self.room_links,
            self.images,
            self.buttons,
            self.dialogs,
            self.error_marker,
            self.challenge_marker
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_release_lists_get_past_seed_admission_with_every_list() {
        assert!(super::release_seed().is_ok());
    }
}
