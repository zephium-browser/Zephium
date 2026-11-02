# Work persistence boundary

The optional application Work coordinator now owns admission and durable
recovery classification. This is not executable session restoration and does
not reopen the engine's single-use agent port.

The optional `zephium-store/work-execution` adapter uses the existing Store
actor and a lazy four-operation admission counter. It starts no worker, timer,
provider or browser. Default Store dependencies do not include the private
filesystem adapter. Meta migration 17 adds empty, bounded Work tables; no Work
namespace or lock is created until explicit claim.

A claim acquires the existing `LockedPrivateNamespace` authority under the
application data directory. Store mints a fresh incarnation itself; callers
cannot restore an old incarnation. The exclusive lease remains held until OS
process exit, including after uncertain transactions and Store shutdown. A
bounded process-static owner prevents replacing a Store from reopening Work
admission while an unclean native engine teardown may still be running. Every operation
revalidates that exact lease. This currently admits macOS/Linux; unsupported
platforms and transient in-memory stores fail closed.

Within one FULL-synchronous SQLite transaction, claim classifies all prior
nonterminal records as interrupted and publishes the new process fence.
Already-terminal records remain byte-for-byte unchanged. A crash, missing
acknowledgement or partial transaction never implies a native mutation did not
occur. Recovered facts cannot recreate refs, native contexts, provider state,
task predicates or executable approvals.

The retained-resource coordinator pauses in `NeedsReview` when claimed history
contains an `Interrupted` record under the new process fence. Its bounded handle
accepts an explicit review of those exact bytes; only the original Store CAS ACK
can classify that record as `FreshAdmissionRequired` or `Rejected`. Both retain
all historical debt. After every interrupted record is reviewed, a separately
prepared objective may acquire the newly constructed native resource and pass
ordinary fresh admission. Historical classification never settles old effects,
reconstructs authority, or reuses an old execution owner. Unreviewed records,
foreign nonterminal fences and unknown-debt `FailedClosed` history still block.
Cancellation, malformed ACKs and uncertain persistence keep admission closed.

For the release-excluded developer runner, set `ZEPHIUM_WORK_REVIEW` to a review
sidecar path. The runner reports exact content-free interrupted record hex while
waiting. An explicit JSON decision `{ "record": "<reported hex>", "decision":
"accept_fresh_admission" }` (or `"reject"`) submits that one review. No file means
no review; stale process/revision bytes cannot match. This is development input
to the same handle, not a model tool or an alternate persistence path.

Each fixed 96-byte record contains only version/disposition/debt bits, a checked
monotonic revision, process identity, manifest/run identity and the existing
content-free manifest guard. No objective, credential, page/provider content,
URL, profile path, screenshot or raw trace is stored. Debug omits identifiers.

### Retention

Every page read is one run, so a Store that kept every run forever filled up
after 1,024 reads and then refused Work for good, across relaunches. Meta
migration 25 (and its twin, migration 2 of the protected Windows Work database)
splits runs in two:

- **Clean terminal runs** (terminal disposition, no recorded debt) are history.
  Each records the order it settled in. The Store keeps the 256 most recently
  settled ones, with their results, and prunes older ones oldest first: at
  Store open (the Windows Work database prunes when it lazily opens), after a
  run ends (at most 64 per pass, after the reply, so nothing waits on it), and
  before a result is published when the 32 MiB result budget needs room (the
  16 newest results are never taken this way, since a run reads its own result
  back). A result that a Work execution still cites as evidence keeps its run.
  Older results were only read back by the run that wrote them.
- **Live or debt-bearing runs** (`Admitted`, `Running`, `Interrupted`,
  `RecoveryRequired`, and any reviewed or fail-closed run whose debt is not
  `NONE`) are recovery history. They are never pruned, and the delete trigger
  still refuses deleting one, whoever asks. Only these count against the
  1,024-run admission ceiling. A claim returns all of them plus the retained
  clean window.

If recovery history ever fills the ceiling, admission is refused rather than
anything being evicted, and the page ends with the closed note "Work storage
needs attention", which the canvas shows instead of a generic read failure.
The Store also refuses admission past 4,096 stored runs of any kind; only
results still cited by Work can hold clean runs beyond the window.

Updates compare exact previous bytes, not just counters. An exact retransmission
can acknowledge an already-committed write, but cannot execute anything. Terminal
facts are immutable in both the core transition grammar and SQLite. Decoding a
fact does not mint a successful terminal mutation: its constructor requires the
original successful policy/audit settlement and native shutdown proof, joined
to the exact manifest guard. The application must additionally retain the
matching clean runtime lifecycle result.

The same constructor-closed boundary now admits distinct immutable `Failed`
and `Cancelled` facts with zero debt when the original policy closure reports
that outcome and the native/runtime owners prove drain. Generic transitions,
review decisions and decoded historical records cannot construct those writes.
`FailedClosed` remains a conservative classification, never itself proof of
clean cleanup; it preserves the prior debt, including a proof-closed review's
zero debt. The fixed 96-byte version-one
envelope adds two closed discriminants; older readers reject these unknown
values rather than guessing success. No schema migration or result body is
needed. Exact CAS, rollback, lost-ACK reconciliation and restart immutability
apply equally to all proof-bearing terminal outcomes.

Accepting review produces `FreshAdmissionRequired`; rejecting it produces
`Rejected`. Both preserve the exact prior execution debt. Neither resumes the original
proposal, and neither means clean resource settlement. Stale/conflicting
decisions fail exact CAS; a new user decision cannot reopen a terminal record.
Any later approved execution needs newly admitted native authority and a fresh
trusted observation.

An exact never-dispatched policy refusal can now use original failed
policy/audit/native/runtime closure to persist zero-debt `NeedsApproval`.
After an exact review decision ACK, `Reviewed` is eligible for explicit fresh
admission only alongside those retained original owners. Unresolved debt and
decoded historical facts cannot qualify. Unreviewed records still become
`Interrupted` with unknown debt on restart. See [closed review](agent-work-review.md).

## Application admission and reconciliation

`zephium-app/work-execution` provides `CallbackHandle::attach_work`,
`PreparedAgentWork` and the content-free `AgentWorkApplicationHandle`. The
composition root supplies the same `Arc<SqliteStore>` allocation for ordinary
Store, Work journal and audit ports, and the same engine owner for ordinary
Shell, attachment and prepared execution. The shell rejects a different owner,
an ordinary second coordinator, or an already-owned legacy agent lifecycle. Attachment
claims the durable recovery inventory, but creates no runtime or native page.

Preparation owns the existing stateless `AgentWorkController`, trusted manifest,
task predicate and provider configuration. The one-shot native factory must
transfer a port from the actual engine's unique native authority; it is not a reset or
reopen operation. Only after exact durable `Admitted` and `Running`
acknowledgements does the shell create the original runtime, invoke that factory
and start the controller. Original deadlines include all persistence waits.

An explicit `attach_successor_work` may replace only the exact previous fully
closed coordinator, joining original lifecycle/native/policy/audit owners to
its acknowledged immutable terminal. Pending outputs/events or any uncertain
owner refuse replacement. The same engine/Store and process fence persist;
the new projection/record/runtime/native lifetime are distinct. See
[sequential lifetimes](agent-work-lifetimes.md). Terminal facts alone, including
decoded historical facts, never authorize this replacement.

The shell retains the runtime lifecycle and move-only outcome. `Succeeded` is
published only after the original controller policy/audit closure, native proof,
clean runtime worker join and durable terminal CAS acknowledgement. A timeout,
refusal or lost callback never substitutes empty owners. Even an undispatched
cancelled controller clears credentials/objective and retains its original
non-executing recovery owner; draining audit alone cannot establish Clean.
If a bounded unclean lifecycle join precedes the worker's final Drop, the
completion wake still transfers that original late recovery outcome into the
application. The earlier unclean result remains unclean.

A fully settled unsuccessful actor outcome uses the same lifecycle/native
join, publishing `Failed` or `Cancelled` only after its exact immutable terminal
write is acknowledged. A lost ACK retains that original CAS for bounded
explicit reconciliation; a late cancellation cannot change its disposition.
No result is released or artifact published for this outcome. Historical
artifact reads are independent of the current run's terminal phase.

The application uses one durable request slot and one recovery audit slot, each
with a two-second acknowledgement deadline and at most four explicit
reconciliation dispatches. Reconciliation can retransmit only the identical
retained CAS or the exact original audit ledger delivery. A newly prepared audit
batch uses the ledger's checked next delivery identity, not a guessed count.
No provider request or native action is replayed. Undelivered audit remains debt;
draining it does not clear native, provider, accounting or lifecycle obligations.

Explicit durable-profile extraction can require an atomic private
[artifact/terminal publication](agent-work-artifacts.md). It shares the same
retained request slot and Store permits, not the content-free journal/audit
body format. Read-only archive failures release their own slot without consuming
future admission; publication uncertainty retains the exact original owner.
A result that cannot be prepared for publication never reached the Store, so
it is not uncertainty: the exact Running row is classified `FailedClosed`
through the same resent CAS, and the page closes like any failed page.

A page whose request is still uncertain after its last resend cannot be
classified in this process. It keeps that request, its debt and its native
seat, because its native audit never precedes the row's classification; a
late acknowledgement still settles it. Its page group therefore cannot close,
so the Shell refuses every page waiting for a seat at once with `Unsettled`
instead of letting each wait out its own deadline. A restart classifies the
row `Interrupted` as usual.

The existing shell mailbox carries one globally coalesced Work wake. No extra
worker or periodic poll is added; the existing timer wakes only for pending
acknowledgement/admission deadlines. The application projection holds at most 64
events plus one retained overflow event, in addition to the controller's original
64-event queue (129 total). Pressure revokes execution without overwriting events;
consumption wakes the shell to drain retained events. Typed state and recovery
inventory contain no objective, page/model content, secrets or native handles.

Review is an exact incarnation/revision CAS, not a model tool approval. Accept
records `FreshAdmissionRequired`; reject records `Rejected`. Cancellation and
review are ordered by the first retained CAS. A replay, stale revision or foreign
application handle cannot admit work. Each coordinator and native port admits
one execution attempt. Fresh approved execution requires an explicitly joined,
fully closed predecessor and a distinct native lifetime, not continuation on
the consumed coordinator or release of the process fence. Restart never recovers a
credential, task predicate, observation/ref or mutation permission.

Deterministic tests cover the complete disposition-pair grammar, malformed
records, overflow, live-owner exclusion, restart classification, partial restart
and write rollback, lost commit acknowledgements, approval/reject/cancel races,
terminal immutability, retention pressure, callback loss/panic and bounded Store
mailbox admission. A real subprocess fixture additionally proves exclusion
before and after Store drop, and lock release after actual child-process exit.
Unit restart injection releases only a test-owned fence with no live Hub;
production has no fence-release operation. Application tests additionally drive
the actual public shell command path with the same SQLite Store for ordinary
storage, journal and audit, using a deterministic typed native fixture. The
fault matrix covers approval acceptance/rejection/replay, cancellation/takeover,
lost native callbacks, uncertain admission/terminal acknowledgements, runtime and
native-factory refusal, bounded wake pressure and exact audit redelivery.

The optional [macOS composition adapter](agent-work-composition.md) now supplies
the actual deferred EngineHost factory and explicit trusted task contract. Two
public Luna runs through that adapter, actual shell and native engine completed
with durable success and clean shutdown; full desktop UI/bootstrap
qualification remains separate. Remaining
seams are trusted product task/plan authoring and user-facing Work state;
persisted facts deliberately cannot automatically resume execution. No new UI,
site tools, platform suspend support or battery qualification is included.


## Product Work authoring (profile schema 15)

The product aggregate now starts at `zephium_core::work::WorkSnapshot`, separate
from the content-free execution journal above. The existing agentic `WorkId`
is re-exported from Core with the same canonical ULID encoding and redacted
Debug representation. Browser resources and orchestration still use that same
identity, not a second product label.

`Handle::work_document` accepts `WorkIntent`, a bounded, one-shot user intent.
Rust mints new Work, question, plan, and node identities and assigns user
attribution before queue admission. Plan proposals use bounded temporary node
keys; neither a model proposal nor the application caller chooses new durable
identities or authorship. Internal Store requests carry the minted IDs and
explicit descriptive provenance. The request receipt retains its Work ID and
the profile selected before Store dispatch even if creation loses its reply, so reconciliation never needs blind
recreation or guessing the owner from the newly focused window.
Shell selects its focused regular profile and refuses private or quarantined
profiles. Store independently checks the current durable registry and degraded
profile state. This Rust-only entry point is not exposed through desktop IPC;
there is no frontend, provider, worker, approval compiler, or execution attached
to it yet. Reply ownership includes the selected profile, which presentation
must recheck before rendering a delayed result.

The supported vertical is create from plain objective, read/list, edit objective,
open/answer clarification questions, replace a structured draft, list retained
plan revisions, and read a prior plan revision. Draft validation checks unique
node/output identities, a bounded
acyclic dependency graph, nonempty output contracts, and content bounds. Output
review requirements distinguish mechanical verification, source-mapped material
needing review, and user acceptance; these are requests, never proof of success.
Every edit is a Work-revision CAS. Changes to objective or clarification context
clear the current draft; active unanswered questions prevent accepting a
replacement draft. Questions record their exact basis and objective revision,
plus active/answered/superseded/dismissed state. Objective replacement supersedes
all current clarifications without destroying their answers. Dismissal is an
explicit edit. Only `current_questions()` participates in planning.
Objective, question, answer, plan, and event attribution distinguishes the user,
primary agent, other agent, and unknown legacy origin. Attribution grants no
authority and does not claim factual correctness. Revisions serialize as
canonical decimal strings to avoid JavaScript rounding. Unknown payload fields and versions are refused, never repaired.

### Persistence decision and recovery

Use the existing per-profile database for these content-bearing facts, extending
its established registration, schema-validation, degradation, and deletion
lifecycle. Separate normalized envelopes retain Work ownership, ordered plan
revisions, node identities/order, questions, status, and timestamps. Node-local
requirements/dependencies and question content use bounded typed JSON under the
owning Work schema version. A content-free semantic event shares every mutation
transaction. This is current-state persistence with immutable plan revisions,
not event sourcing or provider transcript retention.

The alternative of placing Work content in the global execution journal would
mix durable user content with the kernel's process-fenced admission facts.
A separate database/worker would duplicate ownership and deletion protocols.
An opaque whole-canvas document would make presentation the persistence model.
The selected boundary keeps all three independent. Revisit the storage split
when cross-profile export or large immutable artifact publication is implemented;
those require explicit migration and ownership joins.

Profile migrations 14 and 15 are forward-only. Older binaries reject the newer
schema. Migration 15 validates and retains legacy Work, plan, question, and
event content, with unknown legacy provenance. Because v1 did not record a
question's objective basis, migrated questions are explicitly superseded with
unknown basis; their former current plan remains readable as history and must
be replaced before further planning. Works without legacy questions retain their
current plan. The migration uses a fixed v1 reader and v2 writer, independent
of the evolving Work domain structs. The conversion and payload accounting are atomic; malformed
legacy content is preserved by refusing the migration.
Failed transactions roll back state, nodes, byte accounting, and audit together;
a failed commit or lost callback is uncertain and requires read/reconciliation
of the same Work/revision. Replaying an old revision cannot overwrite later
facts. Restart restores authoring state only: `plan_ready` means a draft exists,
not that it is approved or executable. Neither decoding nor history lookup can
create a manifest, native lease, or provider continuation. Profile deletion
scrubs these tables through the existing crash-resumable erasure path.

### Bounds and evidence

Admission allows four retained application submissions and four Store commands;
cloned Shell commands share a single consumable payload. Idle authoring creates
no worker, timer, polling loop, provider session, or native browser context.
Each profile supports 256 active Works, 512 total active/archived Works, and
40 MiB of content payload. The extra 8 MiB accommodates v1 provenance metadata
migration even when the former 32 MiB budget was full. Transactional
SQLite triggers maintain the payload counter without scanning historical bodies
on each edit; normalized metadata is separately bounded by row limits. Each
Work retains at most 32 plan revisions, 64 nodes per plan, 32 questions, and
2,048 semantic events. Exhaustion refuses the edit and retains existing facts.
Archive releases an active slot, restore checks active capacity, and explicit
CAS deletion releases that Work's retained rows and content quota. Archived Works are
readable and refuse content edits until restored.

Explicit `CompactHistory` removes superseded drafts and inactive questions,
keeps the current plan and current clarification provenance, and retains the
latest 64 prior events plus its own event. It works even at the event limit.
Revisions never reset; the persisted audit floor defines a contiguous retained
suffix. Plan/question/event deletion, byte accounting, and the compaction event
commit together. No history is silently evicted. All current plans are
unapproved authoring drafts: the future approval compiler must add durable
retention references before approval/execution is exposed, and compaction and
Work deletion must respect those references. This operation must never be
reused to erase an approved/executed revision merely because it is no longer
current. Lists use stable ID keyset pagination with at most 32 summaries, not a cross-page snapshot guarantee. Full Work reads
are bounded by those aggregate limits. Normal text fields are at most 8 KiB.

Core, Store, and application tests cover graph validation, exact revisions,
profile/private boundaries, real Store round trips, restart, retained plans,
transaction rollback, ambiguous acknowledgment reconciliation, quota refusal,
corrupt payload preservation, migration/rollback refusal, deletion, and bounded
mailbox ownership. This is deterministic authoring evidence, not qualification
of model planning or the full intent-to-execution Work product.

Initial-slice verification on 2026-09-11: the Core (354), application (315), and
agentic (618) unit suites passed. The final Store run with `work-execution` enabled
passed 277 tests and four documentation tests, with one existing ignored test.
The application one-shot settlement regression also passed after its final edit.
The full dependency-inclusive strict Clippy command is not green: it reports
existing `unreachable`, `too_many_arguments`, and `manual_contains` findings in
unchanged agentic code. These findings are not waived by the authoring evidence.
The scoped `--no-deps` strict Clippy run also found a pre-existing
`nonminimal_bool` expression in `work_resources_application.rs`; it reported no
authoring-code lint findings. Formatting and diff whitespace checks passed.

Follow-up verification on 2026-09-11: Core passed 356 unit tests, application
passed 317, and the final Store run with `work-execution` passed 282, with one
existing ignored test. All four Store documentation tests passed. These suites
include 26 focused authoring tests covering the intent, lifecycle, recovery,
retention, provenance, and migration boundaries. The profile-erasure fixture was
updated for v2's required fields before the final full Store rerun.
`cargo clippy --no-deps -p zephium-core -p zephium-store -p zephium-app
--all-targets --offline -- -D warnings` passed for the default authoring build;
this does not supersede the earlier findings in the optional execution build.
Formatting and diff whitespace checks passed. No provider calls or GUI
qualification were used for this authoring follow-up.


## On-demand model-backed planning

`zephium-app/work-planning` adds `WorkPlanningService::plan(profile, id,
expected_revision)`. The service uses the existing Handle/Shell/Store boundary
for both its read and its final write. Shell checks the selected regular,
non-quarantined profile against the caller's displayed owner each time. Store
independently enforces profile eligibility and revision CAS. A newer edit,
archive, deletion, or changed/private owner prevents a delayed proposal from
replacing current facts. The persisted question or draft is attributed to
`PrimaryAgent`; Rust generates its durable IDs.

Core's provider-neutral disclosure selects the objective, current answered
questions, and optional current draft. It excludes historical questions,
profile/Work/question/plan/node IDs, audit events, tabs, files, accounts, and
credentials. Existing draft references become temporary positional keys. Active
unanswered questions and archived Work refuse generation. Text selection is
bounded to 32 KiB before copying; the provider additionally bounds serialized
context and the complete request. Overflow is explicit, never truncation.

The OpenAI adapter owns one nonstreaming Responses call, with `store:false`,
no tools, disabled truncation, and a strict JSON-schema proposal: one clarification
or one bounded DAG. Completion, model/route identity, usage totals/subsets,
output size, graph references, and all domain limits must validate before
minting IDs or submitting a write. Refusals carry usage but no proposal.
Reasoning and raw provider bodies are neither returned nor persisted. Secret
shape screening reuses the existing heuristic; it is not comprehensive DLP.
The wire contract follows the official [Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs).

The adapter reuses the existing redirect-free fixed endpoints, credential
handling, response-header limits, four-slot transport registry, and shutdown
root. Planning slot identities are separate from browser execution identities.
The exact input/schema projection is sent to `/v1/responses/input_tokens` before
generation. Its count must fit the configured input range and cost reservation.
The conservative cost uses the catalog's highest input rate, including cache
writes, and its output rate. Maximum accepted configuration is 32K input tokens,
8K output tokens and $1 per call; actual composition normally chooses less.
The live fixture uses the existing Luna catalog and a $0.10 ceiling; its rates
were checked against the official [Luna model page](https://developers.openai.com/api/docs/models/gpt-5.6-luna).

The application admits at most two concurrent planning operations, with one
per profile/Work. Request futures use callback wakeups, with no polling worker.
Construction and idle ownership create no tasks, sockets or timers. The caller
owns the async runtime and can drop the future to stop local provider I/O.
Dispatched generation that loses its outcome seals the shared transport and
returns unknown billing when it can return at all; no automatic retry or refund
is inferred. Before generation, cancellation leaves no proposal. Once the final
Store edit is queued, it may commit even if the caller disappears. Lost write
acknowledgement remains `OutcomeUnknown` and requires a fresh read under the
same owner, not replay. Provider usage remains available on final CAS failure.

This is a Rust backend vertical, not frontend or execution qualification. It
introduces no database schema change, executable approval, browser authority,
autonomous loop, or persistent model transcript. Planning-attempt accounting is
currently a returned receipt and a process-local fail-stop boundary; durable
attempt/restart reconciliation must precede autonomous scheduling or aggregate
budget claims. The separate Work frontend and approval compiler remain open.

Live qualification on 2026-09-11: the explicitly invoked
`planning_live_openai_through_application_and_reopened_store` test passed using
`gpt-5.6-luna`, medium reasoning, an 8,192-input / 4,096-output token limit, and
a 100,000-micro-USD reservation ceiling. The provider returned 423 input and
580 output tokens; conservative usage pricing was 802 micro-USD ($0.000802).
The five-node draft passed domain validation, persisted as `PrimaryAgent`
through the actual application Handle/Shell/Store path, and matched after
closing and reopening the temporary database. Keychain access required the
user to accept the macOS prompt. The browser engine was a test double; this
qualifies model planning and durable application integration, not native
browser execution, frontend behavior, or general planning quality.

The live test is ignored by default and uses only a public comparison objective.
Run it explicitly with `cargo test -p zephium-app --features work-planning
planning_live_openai_through_application_and_reopened_store --offline --
--ignored --nocapture`. It reads the existing fixed development Keychain item;
it does not accept credential strings through frontend input or log secrets.

Final regression verification: 680 agentic, 321 application, 358 Core, and
282 Store unit tests passed (1,641 total). All 36 documentation tests also
passed, along with formatting and diff whitespace checks. The default run leaves the explicit
live planning test and the pre-existing Store test ignored; the live planning
test passed separately as recorded above. Thirteen focused new deterministic
tests cover disclosure selection, response validation, counting/budgets,
shared admission, cancellation, observer cleanup, profile/revision protection,
provenance, and database reopen. Local HTTP fixtures require loopback-listener
permission; their accepted sockets explicitly use blocking reads independently
of listener polling mode.

Strict scoped Clippy passed for Core and application with `work-planning`
enabled (`--no-deps --all-targets -- -D warnings`). The broader command including
agentic still reports three pre-existing findings in unchanged code:
`agent_provider/request.rs` (`unreachable!`), `work_browser_observation.rs`
(`too_many_arguments`), and `semantic_wire.rs` (`manual_contains`). No lint
allowances were added to bypass them. The earlier optional execution-only
application finding remains outside this planning feature's qualification.

## Local pricing during cancellation and recovery

The trusted lead service records `no_usage_charge` only after resolving all
three chosen models as free Compatible models and selecting browser search
without a provider fallback. Incoming commands cannot assert this fact. Fully
local runs use their chosen light/page client for typed decisions and do not
load fixed paid decision credentials or add a provider-search fallback.

The fact survives Store serialization. Stop, missing terminal usage, lost-driver
reconciliation and restart retain conservative token and operation reservations
with a known zero monetary charge. A page with an in-flight model call uses the
same original admitted pricing fact. This does not confirm native effects or
change cancellation, recovery or cleanup authority. Mixed or paid runs retain paid accounting
and conservative reservations for ambiguous calls; stopping a local call does
not erase charges already measured for other models. Exceeding token or operation
bounds clamps those dimensions without inventing a larger monetary charge.

Older persisted runs default this fact to false. They do not retain enough
trustworthy endpoint and per-role pricing evidence to establish that every
possible call was free. Their recorded charges and unknown reservations remain
unchanged; current endpoint settings or a Compatible label cannot repair them.
A fresh run receives a new trusted pricing classification. Historical billing
would require original authenticated usage evidence before any correction.
