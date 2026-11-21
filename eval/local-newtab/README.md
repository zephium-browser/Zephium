# Local new-tab page experiment — not enabled in the browser

Related to zephium-browser/Zephium#470. Website destinations are addressed in
zephium-browser/Zephium#571. This separate draft investigates what a local HTML
new-tab destination would require. It does **not** add a working local-path
setting or change Zephium's navigation, CSP, extension or filesystem policies.

## Reproduce on macOS

From the checkout root:

```sh
scripts/local-newtab/run-macos.sh
```

Requires Xcode Command Line Tools and a macOS user session with WebKit. The runner
compiles with `-warnings-as-errors`, removes its temporary compiler output, and
opens only nonpersistent, standalone WKWebViews. The program creates dummy HTML
and images under this checkout's `target/`, removes them on exit, and accepts no
personal-file or browser-profile argument. It neither launches Zephium nor adds
an IPC bridge, custom protocol, account, or network service.

## Observed on macOS 26.5.2 (25F84), arm64

The direct-file candidate loaded the dummy HTML and ran its embedded script.
An exact-file `allowingReadAccessTo` argument still allowed a sibling image in
this standalone host. In the reproducible runner, images outside the selected
directory were refused, and an isolated copy in another directory refused both
source-directory and outside images.

The alternative `loadHTMLString(..., baseURL: nil)` loaded the selected HTML's
script, exposed no application bridge, and refused both dummy file-image URLs.
The navigation delegate kept a direct-file page at its selected URL after a
script attempted to navigate to the outside fixture.

See [runner output](macos-output.txt). The positive image control distinguishes
a refused resource from a broken PNG. The experiment includes attempts after a
second view received broader access. An earlier manual build placed its binary
above the fixture tree and allowed the outside images as well; its output is
preserved in [manual-host output](macos-manual-host-output.txt). The runner puts
its binary in a separate temporary directory under `target/`. This difference
is another reason to require qualification of the actual packaged native host,
rather than infer production isolation from one standalone executable.

These are standalone WebKit observations, **not** Zephium/Wry qualification,
proof that arbitrary local bytes are readable, or a production vulnerability.
The probe observes image loading; it does not assert canvas/XHR exfiltration.
Windows/WebView2 has not been exercised. A passing experiment does not authorize
shipping either approach.

## Proposed first implementation for review

Import only the explicitly selected HTML document into an unprivileged, opaque
raw content view. Treat its scripts like other untrusted page scripts. Do not
grant the original directory, expose it through localhost HTTP, or relax global
`file:` navigation. This initial model would support self-contained HTML; local
relative assets would need a separately scoped import design.

Before that can become a usable setting, the implementation still needs:

- Typed, bounded macOS/Windows path validation and a bounded regular-file reader
  outside the application actor, with missing/unreadable/changed-file errors.
- Native/Wry loading of the imported document on **both** supported hosts,
  including hostile subframes, page-created windows, redirects and extensions.
- Exact tab/profile/generation ownership so late reads cannot replace a newer
  navigation or revive a closed tab; proper error, reload and shutdown handling.
- Settings save/clear, startup, private-window policy, restoration and reset
  behavior wired through Rust; no frontend-only navigation authority.
- A decision on local relative assets and whether snapshot semantics meet the
  requested local-start-page workflow.

This draft intentionally keeps those requirements visible instead of presenting
a parser or a standalone probe as completed local-file support.
