# Work engineering guide

Zephium is a native Rust browser with a Tauri + Svelte interface.
Work is part of the default desktop feature set, with a persistent workspace,
conversation, typed objects, execution state and retained browser pages.
Windows and macOS have equal support priority. Linux is unsupported.

This guide describes ownership and contracts for changing Work.
The [browser substrate](agentic-browsing.md) owns native execution details;
[architecture](architecture.md) describes the wider process boundary.
Use [CONTRIBUTING](../CONTRIBUTING.md) for check commands and host requirements.

## Ownership and entry points

| Change | Start here |
| --- | --- |
| Workspace mounting and profile/Space selection | [WorkWorkspace.svelte](../frame/src/app/browser/WorkWorkspace.svelte) |
| Workspace projections and subscriptions | [domain/work-environment](../frame/src/domain/work-environment/) |
| Canvas and object presentation | [features/work](../frame/src/features/work/) |
| Objective-independent workspace relationships | [core work/environment.rs](../crates/zephium-core/src/work/environment.rs) |
| Domain objects, proposals and execution contracts | [core work](../crates/zephium-core/src/work/) |
| Durable commands and projections | [store work_document.rs](../crates/zephium-store/src/hub/work_document.rs) and [work_environment.rs](../crates/zephium-store/src/hub/work_environment.rs) |
| Product execution and settlement | [app work_runtime.rs](../crates/zephium-app/src/work_runtime.rs) and [work_execution.rs](../crates/zephium-app/src/work_execution.rs) |
| Conversation and delegated parts | [app work_lead](../crates/zephium-app/src/work_lead/) |
| Desktop composition and native presentation | [desktop work.rs](../desktop/src/work.rs), [work_product.rs](../desktop/src/work_product.rs) and [work_human.rs](../desktop/src/work_human.rs) |
| Retained browser ownership | [app work_resources.rs](../crates/zephium-app/src/work_resources.rs) |
| Sidebar assistant | [Sidebar assistant](#sidebar-assistant) below |

Keep business admission in Rust. The interface submits typed intentions and
renders projections; it cannot mint an execution permit by changing local state.
IPC contracts live in [zephium-ipc](../crates/zephium-ipc/src/).
Generated interface bindings follow the
[regeneration rules](../AGENTS.md#generated-files-and-forks).

## Workspace and objective identity

The workspace belongs to a regular profile and Space.
[WorkWorkspace](../frame/src/app/browser/WorkWorkspace.svelte) selects that pair,
starts its environment session and stops observing when the selection changes.
Incognito does not acquire the regular-profile workspace through this path.

Environment elements can reference tabs, resources, objectives and immutable
artifacts. Attaching an objective or execution creates a relationship;
it does not grant authority to execute it.
Keep placement and view state separate from the objective's execution state.
Moving an element on the canvas must not rerun its producer.

The domain distinguishes durable identities from process-local capabilities.
A serialized identifier can locate a record, but cannot reconstruct a live
attempt, native lease or approval. Preserve this separation in new IPC fields.

The lead conversation can propose parts and revise canvas objects through
closed tools. [work_lead](../crates/zephium-app/src/work_lead/) admits calls in
Rust and records accepted changes through the attempt so subscriptions can
observe durable progress. Tool output and provider text remain untrusted data.
Do not turn a conversational assertion into an authorization decision.

## Authoring, planning and execution

[Authoring](../crates/zephium-core/src/work/authoring.rs),
[planning](../crates/zephium-core/src/work/planning.rs) and
[runtime](../crates/zephium-core/src/work/runtime.rs) are distinct contracts.
An editable objective, a proposed plan and a running attempt are different
states with different owners. Review the relevant reducer before extending UI.

Commands carry the owning Work identity, expected revision and command identity.
Store admission provides the durable transition before application dispatch.
A stale revision or repeated command must not become a second external effect.
In particular, the public-read path returns a replayed projection without
beginning another node attempt.

[WorkExecutionService](../crates/zephium-app/src/work_execution.rs) binds a fresh
admission to the exact execution, node and attempt. Settlement must match those
identities and the profile. Deserialized receipts and read projections do not
construct executable attempts.

Construction of the runtime is dormant. Starting work requires an explicit
admitted path, not a projection subscription, workspace mount or restored row.
Keep cancellation and waiting bounded; use durable-change notifications in
[work_changes.rs](../crates/zephium-app/src/work_changes.rs) rather than adding
an independent polling loop for every visible object.

## Trusted form transitions

[AgentWorkFormTask](../crates/zephium-agent-controller/src/work_form.rs) admits
bounded fill/select goals supplied by a trusted caller with context, account
and origin. Page content and model output cannot create those goals or grant
execution authority. Local preparation requires independently established
local-only effects; autosave and other remote updates require trusted
external-write assessment. Neither constructor grants submit or navigation.

Fields must resolve uniquely. Transition goals admit the expected prior value
or an already-satisfied destination, refusing other values. Every goal in the
current phase must be independently observed before advancing to the next.
These field contracts are separate from product-level task proposals and approval.

## Confirmation and human decisions

A proposed action cannot approve itself. Keep user confirmation bound to the
current proposal and execution state. Revalidate after edits, navigation or
other changes that invalidate the approved facts.
[execution_proposal.rs](../crates/zephium-core/src/work/execution_proposal.rs)
and [runtime.rs](../crates/zephium-core/src/work/runtime.rs) define the domain
transitions; [work_decision.rs](../desktop/src/work_decision.rs) owns desktop
integration.

Human questions, sign-in and native takeover have their own state.
Presentation does not transfer ownership of a browser page.
Continuing after a human decision requires current evidence and fresh
execution authority; a button press alone cannot prove a site accepted a form
or that an authentication wall disappeared.

If an operation's outcome is unknown, preserve that uncertainty.
Do not retry a potentially completed external write solely because its reply
was lost. Recovery should expose enough state for a safe new decision.

## Retained pages and execution leases

A Work browser resource has stable Work, resource, profile and context identity.
Its [functional contract](../crates/zephium-agentic/src/work_browser_resource.rs)
is separate from task, account, navigation and effect authority.
The application supplies those permissions at the appropriate admission point.

The retained-page owner in [work_resources.rs](../crates/zephium-app/src/work_resources.rs)
coordinates native lifetime. Its controller, application and product modules
join leases, durable admission and selected-profile presentation respectively.
Do not bypass those joins with a direct native view call from the interface.

An execution lease is revocable and run-bound. Ending a lease and destroying
its retained page are different operations. Reuse requires a new admitted
lease and current document evidence; previous references do not survive by
virtue of a stable resource identifier.

Navigation, actions, waits and human handoff have separate resource modules.
Read their tests when changing an asynchronous completion: each completion
must still belong to the current resource, lease and document generation.
Quarantine prevents uncertain native state from being silently reused.

A retained page is not an ordinary tab-strip entry.
Inspecting it must not implicitly adopt it into Browse or enable extensions.
Use the [substrate guide](agentic-browsing.md) for storage and input boundaries.

## Persistence and recovery

[work_document.rs](../crates/zephium-store/src/hub/work_document.rs) owns the
objective document; [work_runtime.rs](../crates/zephium-store/src/hub/work_runtime.rs)
owns durable execution transitions. Environment placement has its own store
owner. Keep changes transactional at the owning boundary.

Restored records describe what was admitted and observed.
They do not restore native handles, live provider calls, browser execution
leases or permission to repeat an effect.
On restart, reconcile in-progress state before admitting fresh work.
A process-local attempt cannot be fabricated from its persisted identity.

The lower-level [agent journal](../crates/zephium-agentic/src/agent_work_journal.rs)
and [application admission](../crates/zephium-app/src/work.rs) serve bounded
controller paths. Their content-free recovery facts are not the complete
product document or an executable session snapshot.
Keep legacy task protocols scoped to their actual callers.

Profile deletion, shutdown and cancellation must settle native work as well as
durable state. A successful store write cannot attest native teardown.
Use the resource shutdown owner and browser lifecycle contracts for that proof.

## Results and evidence

An artifact records a result of an execution; it is not permission to rerun it.
[artifact.rs](../crates/zephium-core/src/work/artifact.rs) and the corresponding
store tests define accepted result shapes and provenance joins.
Preserve the relationship between a finding, its source and its execution.
A result view should expose uncertainty instead of laundering it into success.

Semantic observations, extracted text, screenshots and native completion
receipts have different evidentiary value. Native dispatch success does not
prove the intended business effect. Verification must use current observations
and the admitted postcondition.

Keep raw page content and credentials out of content-free diagnostics.
Follow the media custody types when handling screenshots; do not add generic
serialization to move private native images across boundaries.
The browser guide links the relevant evidence and media owners.

## Sidebar assistant

The sidebar assistant operates on Browse pages and has its own conversation
lifecycle. It is a separate service from the Work lead and retained-page owner,
even where it can explicitly start Work.

[desktop/page_assist.rs](../desktop/src/page_assist.rs) admits main-window
commands and binds platform services.
[work-composition/page_assist](../crates/zephium-work-composition/src/page_assist/)
owns the conversation service, readouts, tools and effect admission.
[zephium-page-assist](../crates/zephium-page-assist/src/lib.rs) is the pure core
for page framing, references, citations, streams, policies, styles and steps.
It owns no I/O, threads or async runtime.

The service keeps a conversation per Space and binds effects to facts actually
read. Committing page steps wait for the person.
Stop, disabling the assistant or closing its conversation must stop activity.
Use the service's admission path for marks, styles and new tabs rather than
calling the engine directly from a tool implementation.

Page-reading adapters are platform-specific:
[macOS](../crates/zephium-engine/src/platform/macos/browse_reader.rs) and
[Windows](../crates/zephium-engine/src/platform/windows/browse_reader.rs).
Page text is data, and stale document references cannot authorize an action.
Keep CSS validation and page-step policy in the pure core; keep native access
in the ports and adapters.

## Changing the contracts

Use adjacent domain, store, application and component tests for the boundary
being changed. See CONTRIBUTING for exact hosted and local check coverage.
A pure reducer test cannot establish native rendering, input or storage behavior.
Windows and macOS native evidence must be evaluated independently.

Read limits from the owning types and tests rather than copying a second
catalog here. The same applies to tool schemas, effect classes and budgets.
Keep qualification evidence under [eval/agentic-browsing](../eval/agentic-browsing/)
and the preserved [Windows handoff guide](agent-handoff-windows.md).
Compilation or fixture success alone does not qualify unfamiliar live workflows.
