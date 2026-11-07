# Browser execution substrate

Zephium is a native Rust browser with a Tauri + Svelte interface.
Windows and macOS have equal support priority; Linux is unsupported.
This guide covers the Rust and native boundaries beneath
[Work](work.md), independently of workspace presentation.

The tree contains functional protocols, product integrations and qualification
adapters. Their existence does not establish equivalent live-site behavior on
both hosts. Native evidence remains in [eval/agentic-browsing](../eval/agentic-browsing/README.md)
and [Windows handoff qualification](agent-handoff-windows.md).
Do not infer production qualification from a protocol type or fixture result.

## Ownership and where to change

| Boundary | Owner |
| --- | --- |
| Context identity and lifecycle vocabulary | [context.rs](../crates/zephium-agentic/src/context.rs), [context_port.rs](../crates/zephium-agentic/src/context_port.rs), [context_registry.rs](../crates/zephium-agentic/src/context_registry.rs) |
| Retained resource and lease protocol | [work_browser_resource.rs](../crates/zephium-agentic/src/work_browser_resource.rs) |
| Observation, references and compact wire data | [semantic_observation.rs](../crates/zephium-agentic/src/semantic_observation.rs), [semantic_runtime.rs](../crates/zephium-agentic/src/semantic_runtime.rs), [semantic_wire.rs](../crates/zephium-agentic/src/semantic_wire.rs) |
| Admission and source-to-sink policy | [agent_policy](../crates/zephium-agentic/src/agent_policy/) |
| Native dispatch, settlement and verification | [semantic_execute.rs](../crates/zephium-agentic/src/semantic_execute.rs), [semantic_settle.rs](../crates/zephium-agentic/src/semantic_settle.rs), [semantic_verify.rs](../crates/zephium-agentic/src/semantic_verify.rs) |
| Controller orchestration | [zephium-agent-controller](../crates/zephium-agent-controller/src/) |
| Execution ownership and native event delivery | [zephium-agent-runtime](../crates/zephium-agent-runtime/src/) |
| Product retained-page composition | [app work_resources.rs](../crates/zephium-app/src/work_resources.rs) |
| Native adapters | [engine platform](../crates/zephium-engine/src/platform/) |

Change a domain invariant in the functional core and its tests before adapting
the shell. Keep native handles, clocks and asynchronous platform callbacks in
the runtime or platform owner. Preserve the [architecture](architecture.md)
privilege boundary between the frame interface and raw web pages.

## Contexts, resources and leases

A context is browser identity, not a disguised tab identifier.
The registry binds operations to the exact context, profile, generation and
navigation state. Asynchronous completions must rejoin that identity before
mutating state; stale completions are refused rather than reassigned.

A retained Work resource has a durable identity independent of actor runs.
Restoring that identity restores no native execution lease.
The resource protocol is not a task, account, navigation or effect capability;
trusted application admission supplies those permissions separately.

A run receives a revocable lease to operate a resource.
Lease drain and page destruction are distinct lifecycle operations.
Keep native cleanup proof separate from durable record settlement.
An uncertain resource must not reenter the reusable pool as if it were clean.
[work_resources_shutdown.rs](../crates/zephium-app/src/work_resources_shutdown.rs)
contains the retained application join for shutdown.

Borrowed tabs and human handoff contexts retain their presentation owner's
identity and viewport rules. Owned-context construction uses a closed viewport
contract from the context port. Do not let page or provider input choose
arbitrary native construction parameters.

## Observation and references

The semantic boundary exposes bounded observations and opaque references.
It does not expose arbitrary JavaScript, selectors, DOM objects, CDP commands
or native handles to the actor.
The [semantic runtime](../crates/zephium-agentic/src/semantic_runtime.rs) and
platform adapters implement fixed internal recipes behind that boundary.

References carry snapshot and document identity.
Locating a control in one observation does not authorize acting on a different
document or a later navigation. Validate freshness again at dispatch.
A readable label, screen coordinate or matching URL cannot substitute for the
required generation joins.

Compact observations and diffs are bounded encodings, not an unfiltered page
export. Secret filtering and source provenance belong before disclosure.
Page text, accessibility data and extracted records are hostile input even
when they resemble instructions or claim to originate from the application.

[semantic_read](../crates/zephium-agentic/src/semantic_read/),
[semantic_extract.rs](../crates/zephium-agentic/src/semantic_extract.rs) and
[semantic_screenshot.rs](../crates/zephium-agentic/src/semantic_screenshot.rs)
own different observation forms. Preserve their source and freshness contracts.
Private native images use [work_browser_media.rs](../crates/zephium-agentic/src/work_browser_media.rs)
custody types; a generic serializable screenshot is not interchangeable.

## Actions and effect verification

An action passes through policy admission, native dispatch, settlement and
independent effect verification. These stages provide different facts.
A native completion establishes what the adapter reports about dispatch;
it cannot by itself establish that a purchase, edit or submission succeeded.

[semantic_execute.rs](../crates/zephium-agentic/src/semantic_execute.rs) splits
one admitted action into retained authority and a closed native recipe.
Its settlement must rejoin the original move-only authority before waiting
or verification can continue. Preserve this one-shot property in adapters.

Bound every wait, retry and batch. A timeout after dispatch may leave an
unknown effect; do not blindly repeat the action.
Use typed refusal and uncertainty outcomes rather than translating every
native error into a retryable failure or successful completion.

Backend support is control- and platform-specific.
Native input must not steal focus or compete with human control.
Do not replace an unsupported native operation with synthetic script execution
that silently changes its trust or activation semantics.
The platform's fixed semantic protocol remains separate from diagnostic or
release-excluded probe facilities.

## Native storage and isolation

Owned Work contexts exclude user extensions and userscripts.
Normal Browse extension behavior does not grant extension access to an owned
resource. Preserve extension-inventory checks at native construction.
See [security-model](security-model.md) for the broader browser policy.

macOS and Windows storage mechanisms differ.
Read the selected-profile construction and transfer code before changing
session behavior: [work_profile.rs](../crates/zephium-app/src/work_profile.rs),
[cookie_transfer.rs](../crates/zephium-agentic/src/cookie_transfer.rs) and
[sign_in_handoff.rs](../crates/zephium-agentic/src/sign_in_handoff.rs).
A cookie transfer is not proof of complete origin-storage equivalence.
Never clone profile directories or widen a scoped transfer to unrelated sites.

The context port intentionally cannot carry cookies, headers, profile paths,
raw platform errors or executable page code. Keep those private to the native
owner. Diagnostics must preserve useful failure classification without leaking
credentials or page content.

## Presentation and human control

Visibility, rendering opportunity, input authority and execution authority are
separate states. Showing a retained page does not create an execution lease,
and hiding it does not prove the platform will continue rendering it.
Do not infer liveness of background pages from successful construction.

Human takeover invalidates prior action authority and references.
Return to automation through current observations and an admitted successor,
not by replaying the interrupted batch.
[work_browser_human.rs](../crates/zephium-agentic/src/work_browser_human.rs) and
[work_resources_human.rs](../crates/zephium-app/src/work_resources_human.rs)
separate native handoff facts from continuation state.
Manual Continue alone does not attest that a sign-in or verification wall cleared.

Inspecting a page and adopting it into Browse are different operations.
Keep ownership explicit across presentation changes and preserve native page
identity where the retained-resource path promises it.

## Lifecycle, evidence and validation

Cancellation must drain admitted work, not merely hide its interface.
[agent_lifecycle.rs](../crates/zephium-agentic/src/agent_lifecycle.rs) and
[agent_native_shutdown_driver.rs](../crates/zephium-agentic/src/agent_native_shutdown_driver.rs)
keep native shutdown authority distinct from descriptive outcomes.
The supervisor, policy, audit and native owners each settle their own state;
a terminal label is not proof that every owner has settled.

Use adjacent functional tests for identity joins, replay refusal, cancellation,
policy and evidence transitions. Use native qualification for rendering,
activation, storage transfer and teardown on each supported host.
Compilation and cross-compilation do not replace either host's native evidence.

The [evaluation README](../eval/agentic-browsing/README.md) owns qualification
entry points and evidence interpretation. Preserve its evidence files and the
Windows handoff guide when changing the implementation.
Fixture adapters may supply trusted workflow facts; success through those
adapters does not establish autonomous discovery of unfamiliar routes.

Read current limits and closed vocabularies from their owning Rust types and
tests. Avoid maintaining a second numeric catalog in documentation.
Use [CONTRIBUTING](../CONTRIBUTING.md) for verified check commands and coverage;
its native qualification requirements apply to browser substrate changes.
