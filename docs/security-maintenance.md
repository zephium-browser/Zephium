# Security maintenance runbook

Zephium deliberately treats the operating-system WebView runtime and the
application build graph as security-sensitive release inputs. The checks
described here fail closed. A red check is an instruction to review evidence,
not a deadline or advisory string to extend mechanically.

## Ownership and cadence

The release security owner reviews the macOS and WebView2 engine channels (and
WebKitGTK, while the Linux code is kept) at least weekly and again immediately before freezing a release commit. A second
maintainer reviews every floor change used for a signed artifact. The review
must happen sooner when Apple, Microsoft, WebKitGTK, RustSec, npm, a supported
distribution, or the Tauri or Wry projects publish a security notice.

Current deadlines are encoded next to their evidence in:

- `crates/zephium-core/src/macos.rs`
- `crates/zephium-core/src/webview2.rs`
- `crates/zephium-core/src/webkitgtk.rs`
- `deny.toml` for temporary advisory exceptions

These commands check the dates. In CI an expired date is a warning; `--strict`
makes it a failure. The weekly `Security review` workflow runs both strictly
and opens an "Engine security review due" issue when either has expired:

```sh
cargo xtask check-engine-floors [--strict]
cargo xtask check-advisory-exceptions [--strict]
```

The release workflow runs the release-only gate, which applies both strictly:

```sh
cargo xtask check-release-engine-security
```

It also fails when a vendor has publicly acknowledged a browser-engine fix
that is not yet available in its supported Stable runtime. Development may
continue on the newest admitted runtime, but an unavailable vendor patch is
never treated as production release evidence.

Calendar alerts should be set for seven days before every encoded deadline.
The alerts are operational backup only; the release gate remains the
authoritative fail-closed control.

These calendar gates are deliberately stricter than installed-runtime
behavior. An overdue review blocks release publication, but does not
make time alone terminate an already installed browser. Runtime refuses only
unsupported or unparseable engines (macOS older than 14, Safari older than 26,
preview or overridden WebView2), provenance failures, security overrides, and
missing mandatory native capabilities, with a native alert and exit status 78.
Falling below the reviewed security floor, behind the newest recommendation,
past the review SLA, or onto a newer stable release line is projected as a
typed, non-fatal advisory, shown as a dismissible sidebar card. Independent facts are retained
together: no severity ranking may discard review age, a patch recommendation,
or a newer unreviewed release line.

Runtime startup never downloads this policy and never scrapes Apple,
Microsoft, or WebKitGTK pages. Those pages are unstable, unsigned inputs from
the client's perspective and would add latency, failure modes, and an
unnecessary first-party network observation. The embedded reviewed policy is
authoritative for startup. If Zephium later refreshes policy independently of
an application release, it must do so asynchronously through Zephium-signed,
rollback-resistant update metadata, persist only an authenticated newer
policy, and retain the embedded hard floor as the offline fallback.

## Native-engine floor review

For each platform:

1. Read the vendor's current security advisory and release-note source, not a
   search result or version aggregator.
2. Identify every vendor-supported operating-system or stable runtime line
   Zephium claims to support. Preview and development channels remain hard
   failures. Decide explicitly whether a newer stable line preserves the
   mandatory native capabilities and can run with an unreviewed-runtime
   advisory; never infer that from an arbitrary numeric version alone.
3. Record the reviewed security floor, newest recommended version, their
   publication dates and source URLs, and the next review deadline together.
4. Confirm startup admission, CI packages, packaged-artifact checks, and the
   documentation all describe the same boundary.
5. Add or update boundary tests for the exact version below the hard floor,
   the floor itself, the newest recommendation, a newer stable line, a
   preview/development line, and the first instant after review expiry. Prove
   that only unsupported or unparseable runtimes reject admission and that
   below-floor, recommendation and review states map to the correct sanitized
   advisory.
6. Run the platform's packaged hostile/native suite. A source-level version
   check is not renderer-confinement or process-mitigation evidence.
7. Preserve links, command output, package metadata, and test artifacts with
   the reviewed release commit.
8. Admitted installations may run below the floor with only an advisory. Do not
   imply in release notes that every installation is fully patched merely
   because the candidate passed its release gate.

If the vendor has disclosed fixes but has not yet published a supported stable
runtime, the release remains blocked. Do not extend the review date merely to
pass the release gate. Development may continue on a documented non-release branch,
but no signed production artifact may bypass the gate.

## Dependency advisories

Rust dependency policy is enforced with the locked graph and `cargo deny`.
An exception in `deny.toml` requires an owner, reason, removal plan, and expiry
within the bounded review horizon. On expiry, remove or upgrade the dependency;
extending an exception is a new security decision and requires review.

The complete pnpm graph is intentionally checked at `high` severity:

```sh
node scripts/ci/audit-dependencies.mjs
```

This includes development dependencies because Vite, package plugins, and
other build tools execute while producing signed artifacts. The only local
exception is GHSA-vfj7-8cjw-p6xm in build-only braces 3.0.3: the command verifies
all affected installed paths against exact patched-file hashes and exercises
hostile patterns and direct ASTs before accepting its depth-limit mitigation.
Other high/critical advisories and registry failures remain blocking. When this check
fails:

1. Confirm the advisory and affected resolved version against the package
   registry/advisory source.
2. Determine whether the dependency executes in development, CI, release
   production, or the shipped application.
3. Upgrade or remove it and regenerate the lockfile deliberately.
4. Re-run the frontend checks and the production build.
5. If no fixed dependency graph exists, keep production release blocked. Do
   not lower the audit severity or omit development dependencies as a release
   workaround.

Registry availability is different from a vulnerability result. A network or
registry failure should be retried from the controlled release environment;
it must not be converted into a passing audit.

## Native-boundary fork maintenance

The in-tree Tauri, Tauri Runtime Wry, and Wry forks are narrow native security
adapters and therefore part of Zephium's trusted computing base. Monitor
upstream security releases and the files touched by every upstream change.
Follow the `REBASE.md` in each fork for every update; a version-only bump or
blind merge is prohibited. Tauri and Tauri Runtime Wry share an upstream
repository and commit, but their complete deltas, lockfiles, and validation
results remain independently reviewable.

Before a stable release, all native-platform checks must run on the immutable
candidate commit. Passing macOS tests does not substitute for Windows or Linux,
and unit tests do not substitute for packaged hostile-page, teardown, erasure,
and endurance tests.

GitHub-hosted macOS images may temporarily lag the embedded hard floor or ship
a Safari application whose build does not match the loaded WebKit framework.
CI uses such an image for source, Clippy, unit, and API-availability coverage
only; it mints no native runtime evidence. Run exact runtime admission and
principal isolation (`cargo xtask ci`) on an admitted Mac before tagging a
release. Never lower a floor, ignore a build mismatch, or relabel hosted source
coverage as release evidence to make CI green.

### Linux

Zephium does not ship on Linux. CI compiles and unit-tests the workspace there
but runs no native WebKitGTK sandbox job and produces no Linux runtime
evidence.

If Linux support returns, the Wayland device pass must also force GlobalShortcuts portal denial
(and separately restart the portal process) and verify that the configured
launcher chord still opens from the focused main window and closes from the
focused launcher panel. After permission is restored, verify one activation per
press, immediate fallback after an authoritative `ShortcutsChanged` removal,
and no activation after browser shutdown begins. Run this against the packaged
artifact whose installed entry is exactly `app.zephium.desktop`; raw `cargo`
or `tauri dev` execution does not prove portal application identity.

A teardown-only GLib-GIO warning from a sandbox child saying that release of an
`app.zephium.Sandboxed.WebProcess-*` bus name failed because its connection was
already closed is a WebKit child-process cleanup diagnostic. Do not hide it
with a GLib log handler or by weakening the WebKit sandbox. It is acceptable
only when Zephium exits cleanly and it is not accompanied by an
`engine: web process terminated` event, a core dump, a hang, or a surviving
auxiliary process. Any of those accompanying symptoms turns it into a native
runtime failure: retain the journal and core, record the exact WebKitGTK/GLib
versions, and keep the candidate blocked pending engine-level investigation.

## Release evidence

For every candidate, retain:

- the exact commit and annotated tag;
- native runtime and OS versions for every runner/device;
- engine-floor and advisory review output;
- the locked dependency manifests (`Cargo.lock`, `pnpm-lock.yaml`);
- packaged hostile/native and profile-erasure results;
- resource/endurance measurements;
- the release workflow run: signing, notarization, entitlement, updater
  signature, and provenance verification output.

Any source, lockfile, floor, exception, build image, signing input, or workflow
change invalidates the prior evidence and requires a new candidate run.
