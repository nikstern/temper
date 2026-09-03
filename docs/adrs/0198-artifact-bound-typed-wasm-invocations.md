# ADR-0198: Artifact-Bound Typed WASM Invocations And Outcomes

- Status: Proposed
- Date: 2026-09-03
- Deciders: Temper core maintainers
- Related:
  - ADR-0157: Metadata-Generated Typed Module Data SDK
  - ADR-0189: Typed WASM State Boundaries
  - ADR-0190: Typed WASM Guest Terminal Failures
  - ADR-0193: IOA Action Parameter Requiredness
  - ADR-0194: IOA-Authoritative Canonical Entity Model
  - ADR-0197: Module-Data Failure ABI v2
  - `crates/temper-spec/src/automaton/`
  - `crates/temper-codegen/src/module_sdk/`
  - `crates/temper-wasm-sdk/`
  - `crates/temper-wasm/`
  - `crates/temper-server/src/state/dispatch/wasm/`

## Context

Temper's generated module-data client binds data access to a canonical schema,
but a module invocation remains application-owned JSON plumbing. A guest reads
an untyped `Context`, interprets `trigger_params` and `entity_state`, chooses a
raw callback action string, constructs an untyped payload, and often owns an
`extern "C" fn run` implementation. Missing fields can be replaced with empty
strings, objects, or nulls. The kernel cannot prove that a successful raw
result is a callback permitted by the trigger or that its payload matches the
callback action's canonical parameters.

ADR-0189 introduced a strict state decoder but deliberately deferred generated
state models. ADR-0190 established exclusive, bounded success and failure
result shapes and a kernel-owned invalid-result failure. ADR-0193 and ADR-0194
introduced IOA/CSDL parameter requiredness, property provenance, and canonical
types for generators. ADR-0197 provides the sound, infallible
`ModuleDataError` conversion needed by a typed handler.

Issue #93 completes this boundary. The artifact must carry the exact invocation
contracts against which it was compiled, and both sides of guest execution must
be validated even when a guest bypasses the generated SDK.

## Decision

### 1. Make Typed Invocation An Explicit Per-Trigger Contract

WASM action triggers gain:

```toml
success_actions = ["ChargeSucceeded", "ChargeDeclined"]
```

The field's presence is the typed opt-in:

- absent preserves legacy `on_success` behavior;
- present and non-empty declares permitted callbacks in declaration order; and
- present and empty declares typed success with no callback.

A typed trigger cannot declare legacy `on_success` or `on_failure`. Typed
`failure_routes` remain optional, and an undeclared failure category continues
to fail closed. Success actions are never inferred from broadly enabled state
machine actions.

All WASM trigger references to one published module name within a canonical
bundle must use the same authoring mode. A module referenced by both typed and
legacy triggers is rejected with a diagnostic requiring the legacy trigger to
move to a separately named artifact or opt in explicitly. Opt-in remains
trigger metadata rather than an app-manifest switch, while each concrete guest
artifact retains one unambiguous entrypoint contract.

For every declared success action the canonical model validator requires that:

- the action exists on the triggering entity and is not the source action;
- it is enabled from every possible committed post-trigger state;
- its ordered parameter names, requiredness, nullability, and types are
  representable by the generated Rust and JSON contracts; and
- actions are unique and their generated names do not collide.

`success_actions` is invalid on `Entity`, `Adapter`, and `Webhook` triggers,
even when it is empty. It is also invalid on legacy top-level `[[integration]]`
declarations; typed invocation bindings require an inline WASM action trigger.

The possible post-trigger states are computed only from the canonical model.
For every effective valid-from state of the source action, its post-state is
the action's target state when present and otherwise that same valid-from state.
If the trigger declares `to_state`, retain only that matching post-state; an
empty result means the trigger cannot fire and is rejected. Every declared
success callback's canonical effective valid-from set must contain the entire
remaining post-state set.

The binding identity is the fully qualified tuple `(entity type, source action,
trigger name)`. Two typed triggers that otherwise look alike remain distinct.
Ambiguous source-action/trigger resolution is invalid.

**Why this approach**: Explicit metadata turns callback authority into reviewed
application behavior. Presence distinguishes an intentional empty outcome set
from a legacy trigger without adding an application-wide mode switch.

### 2. Emit One Versioned Unified SDK Binding

New generated artifacts use an `sdk_binding` sidecar whose outer
`binding_version` is `1`. It replaces new-generation `data_binding` and
contains:

- module name;
- canonical closure, independently recomputable dependency-lock, and schema
  digests;
- generator version;
- the final artifact digest in the external sidecar view;
- generated-symbol digest;
- the existing optional compatibility proof;
- zero or one data ABI-v2 and grant contract; and
- deterministically ordered invocation bindings.

When present, the data portion is a lossless home for the complete current
data-binding activation contract: ABI v2, grant and grant digest, entity and
property/action metadata, the used-symbol set and semantic hashes, stream
capabilities and their embedded digest, and the compatibility proof shared with
the unified binding. A unified binding cannot publish a new ABI-v1 data client.
Data-only artifacts remain valid through this optional portion; no existing
grant, write-policy, result-cardinality, stream, or compatibility verification
is weakened by moving to the unified outer contract.

Each invocation binding contains its fully qualified identity, source entity
state projection, source action parameter contract, success-action contracts,
and the canonical names and types required by runtime validation. Ordering is
lexicographic by fully qualified binding identity, while each binding retains
the declared success-action order.

The existing invocation-context JSON object is versioned additively with a
typed-only `trigger_name` string. This is not a second envelope or a new
transport: legacy producers and guests continue using the existing fields, and
legacy SDK decoding ignores the additive key. For a typed invocation the host
must populate `trigger_name` from the resolved `ActionTrigger.name`; the strict
typed decoder requires it and verifies it together with `trigger_action` and
`wasm_module` against one manifest binding using exact, case-sensitive UTF-8
equality. Empty values, values beyond the canonical 256-byte identifier budget,
unknown tuples, and multiple matching bindings are rejected before execution.
The source action is therefore not overloaded to carry a synthesized identity,
and multiple triggers on the same source action/module remain distinguishable.

The exact new custom-section name is `temper.sdk_binding.v1`. Its embedded
`ArtifactSdkBindingV1` omits `artifact_digest`, just as the historical artifact
view does: the packager serializes the embedded view into previously unbound
WASM, hashes those final bytes, and writes that hash only into the external
`SdkBindingV1` sidecar. Verification recomputes the final-byte hash and compares
it to the sidecar, then compares every other embedded field and semantic digest
to the sidecar. No normalization removes bytes from the artifact hash.

The compatibility transition uses these exact names:

- app/deployment manifests add `wasm_modules.<name>.sdk_binding` and retain
  historical `data_binding` only for old artifacts;
- scoped bundle inputs add `sdk_binding_digest`; canonical bundle hashing uses
  new `wasm_sdk_binding_present` and `wasm_sdk_binding_digest` domain tags;
- persisted schema bundles add `wasm_module_sdk_bindings`, leaving
  `wasm_module_data_bindings` readable for historical records; and
- new artifacts use `temper.sdk_binding.v1`, while the existing
  `temper.module_sdk_binding.v1` remains the historical data-only section.

At every layer, two sidecars, two sections of either name, or an old and new
binding together are invalid. A `temper.sdk_binding.v1` body with an unknown
`binding_version` or unknown fields is invalid. New publication never writes
old names; restart restoration continues to accept a self-consistent historical
chain.

SDK generation is valid when a module has a data grant, one or more typed
invocation bindings, or both. It is rejected when it has neither.

**Why this approach**: Invocation and data capabilities describe one generated
artifact surface. One authenticated manifest prevents independent binding
versions or digests from drifting and gives the host one authority to inspect.

### 3. Generate Closed Invocation Types

The generator emits one closed `Invocation` enum variant per invocation binding.
Variant names derive from the fully qualified entity, source action, and trigger
identity. Every variant owns:

- a binding-specific trigger-parameter struct;
- the generated source-entity member-state struct; and
- an `InvocationIdentity` plus read-only runtime context.

Member state is generated from the canonical model and includes lifecycle
status, ordinary fields, counters, booleans, and lists. Identical source-entity
state contracts are emitted once and reused across bindings. Trigger parameter
types preserve canonical requiredness and nullability; no generated required
member receives a default.

`InvocationIdentity` has private fields and getters for tenant, entity type,
entity ID, source action, trigger name, and module name. Optional agent,
session, and trace metadata remains outside identity in a read-only runtime
context, because its absence must not change stable invocation identity.

All generated names use one deterministic Rust normalization function. Two
distinct canonical identities that normalize to the same symbol are a
generation error, not a suffixing opportunity.

Invocation and result JSON uses exact case-sensitive CSDL wire names and CSDL
types from `CanonicalActionContract`. Generated Rust members use IOA names after
ADR-0193's verified normalization and carry explicit Serde renames to the CSDL
wire names. Member-state JSON remains the ADR-0189 IOA-name envelope rather
than CSDL/OData JSON. State generation maps IOA `status` to the closed lifecycle
enum, `counter` to `i64`, `bool` to `bool`, `string` to `String`, `set` to a
deterministically ordered `Vec<String>`, and `ref` to the generated entity ID
newtype. Action values use the existing module-SDK closed mapper for supported
CSDL primitives, enums, entity IDs, and `failure_v1`; collections recurse over
that mapper. Unsupported types fail generation.

The collision domain is the complete generated module: invocation variants,
state/parameter/payload types and fields, outcome variants, lifecycle and CSDL
types, data-client symbols, helper types, and reserved SDK names. Exact repeated
canonical symbols may be reused only where this ADR says so.

**Why this approach**: A closed enum supports multiple triggers per module while
making exhaustive dispatch possible. Artifact-owned state projections preserve
additive schema compatibility without giving old modules newly added fields.

### 4. Generate Closed Success Outcomes

The generator emits one closed `Outcome` enum. It has one variant with a typed
payload for each distinct canonical success action used by any binding.
Canonical callback identity is `(fully qualified source entity type, action
name)`. Bindings on that same entity that share the same action reuse one
variant. Equal simple action names on different entity types are distinct
canonical actions and therefore distinct generated variants; if their fully
qualified generated variants collide, generation fails. If repeated metadata
for one canonical action disagrees on its contract, generation also fails.
Any other distinct canonical actions that normalize to the same Rust identifier
fail rather than receiving order-dependent suffixes.

Typed bindings with no callbacks use `Outcome::Completed`. The SDK emits the
existing bounded success wire shape:

```json
{"action":"ChargeSucceeded","params":{"provider_id":"p-1"},"success":true}
```

`Completed` emits the exact side-effect-only shape with an empty action and an
empty object payload. Generated code cannot construct an arbitrary action name
or payload key.

The SDK owns two public traits implemented by generated types. `TypedInvocation`
has one required associated binding-version constant and one decoding method
from the SDK's strict owned context envelope. `TypedOutcome` has one method
that returns an SDK-owned `{ action, params }` success value after serialization.
The traits are not Rust-sealed: a hand-written or adversarial guest can
implement them, so they provide an authoring contract rather than authority.
Host validation remains authoritative. Authors write:

```rust
fn handle(invocation: Invocation)
    -> Result<Outcome, GuestFailureDeclarationV1>
{
    // domain work
}

temper_wasm_sdk::typed_module!(handle, Invocation, Outcome);
```

The macro exclusively defines `extern "C" fn run`, reads the host context once,
invokes `TypedInvocation` decoding, decodes the matching enum variant, and
publishes exactly one bounded success or structured typed-failure terminal
result. Existing `Context`, raw `Value`, `temper_module!`, and legacy result
encodings remain public for source compatibility. They are supported authoring
surfaces only for legacy artifacts; packaging does not hide Rust APIs. If a raw
or legacy entrypoint is packaged with a typed binding, strict host validation
still enforces the typed context and outcome contract.

`From<ModuleDataError> for GuestFailureDeclarationV1`, supplied by ADR-0197,
allows ordinary module-data calls to use `?` without application-owned mapping.

**Why this approach**: Compiler-checked outcomes eliminate raw callback strings
for generated authors, while host validation below prevents raw guests from
bypassing the same artifact contract.

### 5. Strictly Validate Context Before Guest Execution

The invocation context remains the same top-level JSON object envelope; typed
action invocation adds only the `trigger_name` key defined above. For a
version-1 typed binding, the server resolves the exact binding from the source
action and trigger identity before entering WASM. It then:

1. projects member state to the artifact-bound field set;
2. validates lifecycle, ordinary field, counter, boolean, and list shapes;
3. validates required and nullable trigger parameters;
4. rejects unknown application trigger parameters; and
5. supplies all required identity fields without fallback defaults.

Fields added after artifact compilation are not exposed. A bound field that is
missing, ambiguously mapped, or type-incompatible fails closed. Optional
runtime observation fields outside identity may remain absent.

Compatibility proofs extend the existing semantic-hash comparison by symbol
class. The new `used_symbol_hashes` domain contains separate framed keys for
the binding identity and possible post-states, every bound state member and its
category and type, every trigger parameter and its CSDL wire
name/type/nullability, each permitted fully qualified success action, and every
success parameter contract. Binding identity, possible post-states, permitted
success-action sets, and source trigger-parameter sets must remain exact; a new
source parameter would otherwise be unknown to the old artifact. Every old
state-member key and hash must remain present and equal, while additional state
members are allowed and projected away. Every old success parameter must remain
compatible under ADR-0193; a new required success parameter invalidates reuse,
while an added explicitly nullable/default-compatible parameter may be omitted
by the old payload. Required-to-nullable widening follows ADR-0193, and
nullable-to-required narrowing fails. Removing or incompatibly changing any old
field, action, parameter, post-state, or outcome also fails. The proof and
embedded semantic hashes are covered by both binding and bundle identity.

Any failure becomes a kernel-owned `InvalidInvocationContext` envelope with an
integrity category, `not_applied` outcome, and non-retryable guidance. Guest
code has not started, so the kernel can soundly state that no guest side effect
occurred. Its provenance is `source = wasm`,
`component = wasm-context-validator`, with a closed validation-reason
`source_code`. It dispatches the typed trigger's verified `integrity` failure
route when declared; otherwise existing undeclared-category fail-closed
behavior applies.

**Why this approach**: Strict host validation protects all guests, minimizes
untrusted work before rejection, and gives old artifacts additive-schema
compatibility without weakening current contracts.

### 6. Validate Successful Results After Guest Execution

After guest execution, every successful result for a typed binding is checked
against that binding, regardless of whether it came from generated SDK code or
a raw guest. The host requires that:

- the callback is in the binding's declared success-action set;
- `params` is an object with exactly the canonical parameter names;
- required values are present and nullable values follow metadata;
- every value has the canonical scalar or collection shape; and
- unknown values are rejected.

A no-callback binding accepts only the exact empty-action, empty-object payload.
Typed triggers do not apply legacy static `on_success` precedence; they dispatch
only the validated generated outcome. Legacy artifacts retain current result
and static-callback precedence unchanged.

An invalid typed success becomes kernel-owned `InvalidGuestSuccessResult` with
an ambiguous category, unknown outcome, and reconciliation retryability. Guest
code and host calls may already have performed external effects, so the kernel
cannot claim `not_applied`. Its provenance is `source = wasm`,
`component = wasm-success-validator`, with a closed validation-reason
`source_code`. It dispatches the verified `ambiguous` route when declared and
otherwise fails closed.

ADR-0190 validation runs first. Transport/source-cardinality failures,
oversized bytes, invalid UTF-8 or JSON, and values that do not satisfy its
closed generic success shape remain `InvalidGuestResult` with ADR-0190's closed
source code. `InvalidGuestSuccessResult` applies only after ADR-0190 has
accepted a bounded syntactically valid success object and artifact-aware
validation rejects its action, payload object, names, nullability, types, or
no-callback form. Merely asserting `success: true` does not bypass the generic
parser tier.

This post-parser specialization explicitly refines ADR-0190: ADR-0190 continues
to own generic terminal transport and syntax failures under stable code
`InvalidGuestFailureResult`; a syntactically valid success that violates an
active artifact binding uses the more specific stable code
`InvalidGuestSuccessResult` and the same sound unknown/reconcile semantics.

**Why this approach**: Generated types improve authoring but are not a security
boundary. Artifact-aware host checks make raw and generated guests obey the
same callback authority.

### 7. Bound And Redact Typed-Binding Telemetry

Invocation telemetry may include only bounded control metadata by default:

- binding version;
- fully qualified binding identity;
- validation phase (`context` or `success`);
- stable failure code; and
- validated callback action, when one exists.

Guest diagnostics, trigger values, state fields, and callback payload contents
are not exported by default. Existing tenant-safe cardinality controls continue
to apply. Module, entity, action, trigger, and callback names are each bounded
to 256 UTF-8 bytes at canonical validation. The rendered binding identity is
therefore capped at 770 bytes including separators; telemetry adapters must
reject rather than truncate an impossible over-budget value.

**Why this approach**: Operators need to distinguish binding and validation
failures without turning application payloads or guest diagnostics into an
observability data path.

### 8. Preserve Non-Identity Runtime Capabilities

The generated `InvocationRuntimeContext` owns integration configuration and
optional agent ID, session ID, trace ID, workflow-root entity type and ID, and
workflow run ID. Its fields are private with borrowed getters; it is separate
from `InvocationIdentity` and cannot alter binding selection. Missing optional
observation fields remain `None`, and missing configuration remains an empty
map. When any is present, its JSON type is strict.

Typed action-trigger invocations require `http_request` to be absent because an
HTTP endpoint is not an action-trigger binding. Existing host functions for
HTTP, secrets, logging, spans, time, and module data remain available; the
typed boundary changes invocation data ownership, not host capability grants.

**Why this approach**: Typed domain input should not discard configuration,
correlation, or governed host capabilities that existing modules rely on; it
only separates them from stable identity and makes their decoding explicit.

## Rollout Plan

1. **Phase 0 (Kernel)** — Add metadata validation, unified binding packaging,
   generated types and SDK adapter, strict host validation, compatibility
   readers, fixtures, and local hot-deploy/restart/replay proof. Deploy the
   kernel without production typed application artifacts.
2. **Phase 1 (Validation application)** — Publish a temporary generated app to
   an isolated tenant. Exercise every typed success, domain rejection,
   malformed raw output, and typed failure path; verify callback transitions
   and bounded Datadog traces/WideEvents.
3. **Phase 2 (Downstream ARC repository)** — In a separate repository effort,
   migrate two real modules with different outcome sets and delete their manual
   entrypoint, state decoding, raw result, and module-data error mapping code.
   Keep issue #93 open until this criterion is complete.

## Readiness Gates

- Parser/model tests cover opt-in presence, the empty outcome set, legacy-field
  conflicts, existence and enabled-state checks, ambiguity, duplicates, and
  generated-name collisions.
- Generator goldens and compile fixtures cover multiple bindings, reusable and
  distinct state/parameter types, lifecycle/counter/boolean/list fields,
  multiple outcomes, `Completed`, and invocation-only modules.
- SDK tests cover strict decoding, immutable identity getters, macro entrypoint
  behavior, every outcome, and canonical module-data `?` conversion.
- Host tests cover malformed contexts, wrong shapes, additive projection, raw
  bypass attempts, callback/payload mismatch, budgets, routing, decision IDs,
  deterministic replay, and historical artifact compatibility.
- Two different generated WASM fixtures hot-deploy locally, execute every
  success/failure branch, survive restart, and replay deterministically.
- Workspace tests, strict clippy, rustfmt, dependency, readability, integrity,
  DST, and code-quality reviews are clean.
- The kernel and isolated validation app are deployed and live behavior plus
  Datadog evidence match the artifact contract.
- Maintainers reconcile or explicitly accept the Proposed foundations
  ADR-0189, ADR-0193, and ADR-0197 before accepting this ADR; implementation
  does not silently promote their status.

## Consequences

### Positive

- Module authors work with generated domain types rather than boundary JSON.
- Callback action names and payloads become compiler checked and host enforced.
- Additive schema changes remain compatible with already-built artifacts.
- Multiple triggers can share one module without open-ended dispatch logic.
- Raw guests cannot bypass an artifact's typed callback authority.

### Negative

- The kernel carries historical and unified binding readers during migration.
- Unified bindings increase artifact metadata size and verification work.
- A schema/action rename requires regeneration even when the underlying JSON
  shape is otherwise compatible.
- Strict failures can surface latent producer bugs that legacy defaults hid.

### Risks

- Generator and host validation could diverge. Shared canonical contract types,
  cross-layer fixtures, and raw-bypass tests mitigate this.
- Naming normalization could accidentally alias symbols. Generation rejects
  collisions globally before emitting source.
- Misclassifying a post-execution validation failure as not applied could cause
  duplicate effects. The closed failure mapping always uses unknown/reconcile.
- Rolling the kernel back while typed artifacts remain active would make them
  unreadable. Operational rollback order is enforced below.

### DST Compliance

- Binding and action collections use `BTreeMap`/`BTreeSet` or explicitly sorted
  vectors with stable keys.
- Binding resolution, projection, validation, result mapping, and telemetry
  metadata are pure functions of artifact metadata and invocation input.
- No wall clock, randomness, environment reads, filesystem access, unordered
  iteration, or background task scheduling is introduced in simulation-visible
  paths.
- Replay tests compare exact result and failure classifications across restart.

## Non-Goals

- Encoding application domain decisions in the kernel.
- Inferring callback actions from reachable state-machine actions.
- Introducing a second invocation-context transport envelope.
- Removing historical artifact execution in this effort.
- Modifying TemperPaw or the ARC repository as part of the kernel PR.

## Alternatives Considered

1. **Infer all enabled post-state actions** — Rejected because broad state
   reachability is not callback authority and changes silently as specs evolve.
2. **One handler export per trigger** — Rejected because a module may implement
   several bindings and the existing host invokes one stable `run` export.
3. **Parallel data and invocation binding sections** — Rejected because their
   shared schema, generator, symbols, and artifact digests could drift.
4. **Trust generated SDK output** — Rejected because raw or compromised guests
   can emit arbitrary bytes and must not bypass host enforcement.
5. **Add a new context wire envelope** — Rejected because the current transport
   already carries the required facts; strict artifact projection is the
   missing contract.
6. **Suffix colliding Rust names** — Rejected because generated identity would
   depend on incidental declaration order and obscure canonical ambiguity.

## Rollback Policy

Historical artifacts continue to run throughout rollout. New typed artifacts
require a kernel that understands unified binding version 1. Before rolling the
kernel back, remove or roll back every typed application artifact and confirm no
active schema bundle references a unified binding. The kernel-only deployment
in Phase 0 creates no production typed application dependency. If validation
fails before typed artifacts are published, revert the kernel normally while
retaining historical readers and ABI-v1/v2 data compatibility.
