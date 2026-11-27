# Zephium documentation

Code and tests are the evidence of present behavior. Where documents appear to
disagree, `security-model.md` controls claims about enforced security,
`architecture.md` controls the browser foundation, and `product-system.md`
controls product intent.

## Architecture and security

- [Architecture](architecture.md): implemented browser structure, domain ownership and native lifecycle.
- [Security model](security-model.md): the guarantees the tree enforces and the gates for disabled capabilities.
- [Product system](product-system.md): what Zephium is, the first-release scope and the target architecture.
- [Frontend contract](frontend.md): stack, ownership, import boundaries and native presentation rules for `frame/`.

## Browser features

- [Interface design](frontend.md#native-surfaces-and-presentation): why the browser chrome is shaped the way it is.
- [Interface system](frontend.md#interface-rules): tokens, shared components and how to inspect them.
- [Settings](frontend.md#ownership-ipc-and-lifetimes): settings structure, search and navigation.
- [Launcher](architecture.md#6-history-navigation-and-launcher): the floating search window and what it hands off.
- [History](architecture.md#6-history-navigation-and-launcher): the history module, omnibox queries and site icons.
- [Tasks](frontend.md#interface-rules): the shared Tasks surface for people and agents.
- [Charts](frontend.md#charts): the chart kit used by Work results and Activity.
- [File workflows](file-workflows-windows-qa.md): upload and download ownership, recovery, and Windows acceptance checks.

## Work and AI

- [Agentic browsing](agentic-browsing.md): the browser-execution layer behind Work and how it is qualified.
- [Bounded browser session](agentic-browsing.md#contexts-resources-and-leases): the semantic locate/act controller session.
- Work runtime subsystems:
  [execution](work.md#authoring-planning-and-execution),
  [scoped runtime](work.md#authoring-planning-and-execution),
  [composition](work.md#ownership-and-entry-points),
  [native lifetimes](work.md#retained-pages-and-execution-leases),
  [persistence](work.md#persistence-and-recovery),
  [retained resources](work.md#retained-pages-and-execution-leases),
  [discovery](agentic-browsing.md#observation-and-references),
  [inspection](agentic-browsing.md#observation-and-references),
  [retained inspection evidence](work.md#results-and-evidence),
  [native actions](agentic-browsing.md#actions-and-effect-verification),
  [retained actions](work.md#retained-pages-and-execution-leases),
  [forms](work.md#trusted-form-transitions),
  [results](work.md#results-and-evidence),
  [artifacts](work.md#results-and-evidence),
  [evidence](work.md#results-and-evidence),
  [review](work.md#confirmation-and-human-decisions).
- [Evidence tooling](../eval/agentic-browsing/README.md): committed qualification manifests and the probes that produce them.

## Extensions

- [Using extensions](extensions-user-guide.md): installing from the Chrome Web Store and what to expect.
- [Initial release scope](extension-release-scope.md): what extension support covers and its completion criteria.
- [Windows extension QA](windows-extension-qa.md): the isolated QA build, acceptance checklist and performance checks.
- [Windows extension lab](../crates/zephium-webext-windows/README.md): native qualification harness.

## Ad blocking

- [Ad and tracker protection](adblock.md): the native network blocker, cosmetic hiding and release gates.
- [Cosmetic index delivery](adblock-cosmetic-index.md): keeping the generic cosmetic index out of every page.
- [Blocker fuzz harness](../crates/zephium-blocker/fuzz/README.md): running the fuzz targets.

## Contributing and maintenance

- [Contributing](../CONTRIBUTING.md): prerequisites, workflow and quality bar.
- [Security maintenance](security-maintenance.md): recurring security and engine-floor checks.
- [Release engineering](../.github/RELEASE.md): building, signing and publishing releases.
- [Frame guide](../frame/README.md): layout of the Svelte frontend.
