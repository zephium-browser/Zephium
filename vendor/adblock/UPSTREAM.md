# Vendored adblock-rust provenance

This directory is derived from Brave's `adblock-rust` 0.13.2 tag at the
immutable upstream commit
`00b19a06508ddd4f3453779f8618983421ddf32b` (Git tree
`17f03b25091db8ce9ef88408e4be9d11f3e6f755`):

<https://github.com/brave/adblock-rust/commit/00b19a06508ddd4f3453779f8618983421ddf32b>

`FORK.toml` is the machine-readable companion to this document. The crate is
an audited fork, not an unmodified registry dependency. The complete update
procedure is in [`REBASE.md`](REBASE.md).

## Imported material

`Cargo.toml.orig` is the byte-for-byte upstream source manifest. `Cargo.toml`
is the normalized standalone fork manifest, and the adjacent `Cargo.lock`
pins the fork's independent validation graph. Application builds consume the
fork through the root manifest patch and root lockfile; the fork is
deliberately excluded from the root workspace. The Rust source, benches,
tests, README, and license began as files from the recorded Git tree. The
paths named in `FORK.toml` contain Zephium changes.

[`UPSTREAM_FILES.toml`](UPSTREAM_FILES.toml) records every imported source,
bench, test, and selected support file with both its upstream Git
blob identity and the SHA-256 of its raw bytes. The security-fork gate rejects
missing or added code files, unsafe paths, symlinks, changed support files,
unrecorded source changes, and patch declarations
that no longer differ from upstream. The fork root is closed to unreviewed
entries, so any `data` directory is rejected; only Cargo's real, non-symlink
generated `target` directory is ignored and excluded from source packages. The
gate also records the crates.io archive SHA-256 as import evidence. The
ordinary offline gate verifies that reviewed value and every selected file
identity; fetching and hashing a fresh archive or Git tree is deliberately
part of the isolated rebase procedure, not a network dependency of every
build.

The inventory anchors clean-upstream bytes and the exact union of locally
divergent paths. It intentionally does not duplicate the current byte hash of
every patched source file: those contents are anchored by the immutable
Zephium commit and reviewed as code, while this gate proves that no imported
change can escape the declared patch surface. The active fork manifest is the
one separately reviewed exception; the gate constrains its target layout,
features, dependency names and sources, and test-only resolver patch so it
cannot redirect compilation outside the inventoried source tree.

No upstream `data/` payload is retained. Fixture-dependent upstream tests and
benches are excluded, while retained tests use deterministic Zephium-authored
`.invalid` vectors and generated corpora. Production filter sources remain
separate authenticated packages with an independent redistribution and
license review. Other upstream
repository configuration, release/security documentation, examples, fuzz
targets, JavaScript bindings, npm metadata, the upstream toolchain pin, and
all upstream test datasets are intentionally excluded. The immutable upstream
commit linked above remains the source for omitted material.

Cargo's automatic integration-test discovery remains disabled. The manifest
declares one explicit `fork_contract` target at
`tests/ublock-coverage.rs`. It uses self-authored reserved-domain rules to test
exact attribution, Zephium's conservative unknown-attribution behavior,
preparation budgets, typed failure outcomes, and serialization. CI runs that
target under every shipped graph and the optional
exact-attribution graph; no retained test file is merely assumed to run.

The adblock-rust source is licensed under MPL-2.0; the exact upstream license
is retained as [`LICENSE`](LICENSE). The public fork tree contains no imported
filter-list or replacement-resource payload. Any production filter source
still needs an independent redistribution/license review and must enter
through the authenticated source-package design.

## Security-relevant divergence

Zephium uses adblock-rust in a narrower role than the upstream public API:
network-policy parsing and matching only. The application adapter rejects
cosmetic rules, redirect resources, scriptlets, CSP mutation, URL-parameter
rewriting, generic-hide controls, and tags before it publishes a native
artifact.

The fork adds the runtime properties required by WebView2's synchronous
request callback:

- Regex-backed filters are compiled transactionally under explicit count and
  byte budgets before the matcher is published. A failed preparation cannot
  leave a partially prepared matcher that a retry mistakes for complete.
- A published matcher is frozen. Matching does not compile or evict regexes,
  perform filesystem or network I/O, or wait for a contended lock. Lock
  contention and malformed input return an unavailable result so the native
  adapter can fail open.
- Requests without an exact initiating frame use a distinct
  source-independent constructor and matching path. Source-domain and
  first/third-party predicates are never evaluated against fabricated
  context. That path parses the requested URL without a public-suffix lookup.
  A potentially matching attribution-sensitive exception prevents a normal
  generic block; source-independent `$important` semantics remain explicit.
- Attribution-sensitive exception filters are indexed by requested-resource
  tokens (or the zero bucket), rather than only by an initiating-domain token,
  so conservative exception detection remains possible without a source URL.
- A single aggregate filter-check budget bounds all candidate buckets visited
  by one prepared match. Exact-attribution matching shares it across block,
  exception, redirect, and remove-parameter scans; source-independent
  matching shares it across block, exception, and conservative attribution
  checks. Exhaustion has a distinct typed outcome and the native adapter fails
  open.
- An empty `$removeparam` set returns before query-parameter parsing. Zephium's
  compiler rejects every `$removeparam` rule, so hostile query strings cannot
  trigger that otherwise unnecessary callback-path work.
- WebKit conversion derives `$domain` from the parser's final option boundary,
  normalizes native domain predicates to bounded lowercase ASCII IDNA, and
  fails conversion for invalid domains instead of emitting wider policy.
- WebKit conversion represents internal ABP separators with an explicit ASCII
  byte class. A final separator emits separate separator-byte and exact-end
  rules because WebKit rejects regex alternation; callers must include both in
  their rule and byte ceilings.
- Prepared regexes are built with `regex-automata`'s meta engine directly,
  with the limits, match kinds and byte syntax `regex::bytes` applies, but
  with its fully compiled DFA and capture slots switched off. `regex` builds
  a full DFA for every small regex whenever any crate in the graph enables
  `regex-automata/dfa-build`, and the desktop graph does through `bstr`. With
  the shipped seed the full DFAs held 12.2 MB of the runtime matcher's
  40.5 MB and 2.5 MB of the badware navigation matcher's 7.6 MB. The lazy DFA
  matches the same rules: on a 475-request page corpus the decisions were
  identical, compile time fell from 1.13 s to 0.59 s, median match time was
  unchanged, and the 99th percentile rose 2% for exact and 4% for
  source-independent attribution. The switch is explicit in the
  configuration, so a feature change elsewhere cannot bring the full DFA back.

The changed filter-bucket representation is serialization-incompatible with
upstream 0.13.2. This fork increments the internal data version from 5 to 6
and rejects earlier artifacts. Stable serialization hashes are fork
compatibility sentinels, not values to update mechanically.

Zephium's shipped feature graphs deliberately disable adblock-rust defaults:
Windows uses `full-regex-handling,css-validation`; macOS and Linux use
`full-regex-handling,css-validation,content-blocking`. Static cosmetic admission
uses the upstream CSS parser; unchecked selectors are never treated as validated. The optional exact-attribution graph
adds `embedded-domain-resolver`, but the desktop does not currently ship that
graph. The provenance gate checks both the manifest declarations and Cargo's
resolved desktop graph for Windows, Linux, and both shipped macOS
architectures; workspace inheritance, target-specific overrides, or aliases
therefore cannot silently add the resolver, WebKit converter, or upstream
`single-thread` default to a shipped target. Standalone checks,
Clippy, the full library suite, and the explicit fork-contract target run
against each shipped graph without the upstream `single-thread` feature. A
test-only addr-backed resolver keeps self-authored exact-attribution tests
meaningful in no-embedded-resolver builds; production behavior still requires
an explicit resolver before exact-attribution request construction.

The fork lockfile has a larger test graph than the application lockfile.
Every registry package reachable from the union of shipped adblock feature
graphs must nevertheless resolve to the same version, source, and checksum in
both locks. The security-fork gate enforces that equality across normal and
build dependency edges rather than relying on a periodic manual comparison.
CI also runs the repository dependency policy against the standalone
manifest, so fork-only dev dependencies which execute during tests receive
the same advisory, license, and source review. The standalone command allows
only `advisory-not-detected`, because root-graph exceptions are intentionally
absent from the smaller fork graph; an advisory which is actually present
still fails.

These changes do not make the full upstream feature set a Zephium guarantee.
The product adapter and native engine capability report are authoritative.
See [`../../docs/adblock.md`](../../docs/adblock.md) for the implemented scope
and current enablement gates.

## Current product status

The desktop composition root starts from the immutable, release-authenticated
EasyList, EasyPrivacy and uBlock Origin seed documented in
`assets/blocker-seed/v1`. It validates the exact compressed and raw bytes,
approved CC-BY-SA-3.0 and GPL-3.0-only metadata, and upstream provenance
before the bounded compiler can consume them. New
profile preferences default to disabled; an explicit enable installs the
exact platform artifact before raw navigation is admitted.

No production TUF trust root, repository identity, or fixed origins are
provisioned yet. Bundled mode is therefore network-inert and identifies its
authority as `ReleaseBundle`; only a new signed application release can
replace the lists. Production TUF provisioning, packaged native enforcement
tests, redistribution approval, resource/endurance evidence, sustained
fuzzing, and external review remain stable enablement gates.
