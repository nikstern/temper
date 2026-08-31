# ADR-0194: IOA-Authoritative Canonical Entity Model

- Status: Accepted
- Date: 2026-08-31
- Deciders: Temper core maintainers
- Related:
  - ADR-0157: Metadata-Generated Typed Module Data SDK
  - ADR-0179: Canonical Entity-Valued Action Results
  - ADR-0180: Local-First Immutable App Bundles
  - ADR-0182: App-Rooted Module Binding Verification
  - ADR-0185: Canonical Schema Default Materialization
  - ADR-0186: Canonical Property Provenance
  - ADR-0193: IOA Action Parameter Requiredness
  - GitHub issue #91: Derive CSDL state-machine metadata from authoritative IOA specifications
  - `crates/temper-spec`
  - `crates/temper-codegen`
  - `crates/temper-jit`
  - `crates/temper-verify`
  - `crates/temper-server`

## Context

Temper currently authors the same behavioral contract in IOA and CSDL. IOA
declares states, initial state, actions, guards, effects, references, and
invariants. CSDL separately declares state-machine annotations, lifecycle
defaults and enums, action bindings, wire types, nullability, and return
shapes. Cross-validation catches only some disagreements, while verification,
runtime tables, generated SDKs, and served metadata can still consume
different representations.

Immutable scoped bundles and metadata-generated module SDKs make that split
unsafe. A bundle identity is meaningful only if every production consumer sees
one linked entity contract and if equivalent source formatting produces the
same emitted metadata and digest. At the same time, existing v1 bundles are
durable deployment inputs and cannot be silently reinterpreted by a new
canonicalizer.

## Decision

### IOA is the behavioral authority and CSDL is the wire authority

Every behavioral IOA automaton names its lifecycle property explicitly with
`automaton.lifecycle_property`. IOA owns state order, initial state, action
identity, lifecycle behavior, guards, effects, invariants, reference semantics,
and parameter requiredness. CSDL owns entity structure, binding, exact OData
parameter and result types, nullability, and return shape.

Bundle v2 requires the explicit lifecycle property. The frozen v1 path may use
its existing deterministic enum/default inference so persisted v1 bundles keep
their established meaning. A pure data-only bundle remains invalid, while
data-only entities in a mixed bundle remain valid and receive no generated
behavioral annotations.

For each entity with IOA, the linker requires exact parity between callable
bound CSDL actions and IOA input actions. IOA output actions are excluded.
Unbound actions and functions remain outside the behavioral contract.
Parameter names retain the existing deterministic normalization and collision
rules. An explicit IOA semantic type must be compatible with the CSDL wire
type; neither side silently replaces the other's authority.

**Why this approach**: behavior stays executable and verifiable in one source,
while OData retains responsibility for its precise external representation.

### Bundle compilation produces one immutable canonical model

`temper-spec` defines a `CanonicalSpecModel` keyed by fully qualified entity
name. Each behavioral entry contains stripped structural CSDL, its parsed
canonical `Automaton`, the linked lifecycle property, ordered lifecycle states,
initial state, bound action wire contracts, effective valid-from states, target
state, and the canonical emitted CSDL. The model also retains data-only
structural entities.

Effective valid-from states are the IOA declaration-ordered intersection of an
action's `from` states and every lifecycle `state_in` guard. An unconstrained
action receives every declared state. Duplicate or unknown states fail linking.
The IOA parser preserves state declaration order; no consumer reconstructs it
from a set or CSDL enum.

Bundle compilation is the only production linking boundary. Verification,
JIT `TransitionTable` construction, SDK generation, activation, and metadata
emission consume the model's parsed automata and linked contracts. The TLA+
`SpecModel` remains an explicitly named legacy/test-only API and is excluded
from deployment and generation paths.

**Why this approach**: parsing and semantic linking happen once, so downstream
components cannot validate or interpret subtly different contracts.

### Canonical CSDL is erased and regenerated before v2 hashing

The v2 compiler semantically validates any authored legacy behavioral
projection before removing it. Collections such as states and valid-from
states compare as duplicate-free sets; scalar initial states, target states,
defaults, and enum values compare exactly. Partial matching projections are
accepted, but any contradiction fails with a deterministic entity-qualified
diagnostic.

After validation, the compiler erases all legacy behavioral annotations,
lifecycle defaults, and lifecycle enum members and emits the complete
projection:

- entity `States` and `InitialState` annotations;
- action `ValidFromStates` and `TargetState` annotations;
- the lifecycle property's default equal to the IOA initial state; and
- lifecycle enum members in IOA order with zero-based ordinal values.

A lifecycle enum type must be dedicated to compatible lifecycle properties.
Sharing is permitted only when every linked IOA has identical state order and
the enum is not also used by an unrelated structural property.

Canonical XML emission uses deterministic namespace and declaration ordering,
independent of input enumeration and equivalent formatting. Annotation-free
input, matching legacy input, and already canonical emitted input therefore
produce identical v2 bytes and digests.

**Why this approach**: contradiction checks protect migrations, while erasing
and regenerating prevents legacy formatting or partial annotations from
becoming durable identity.

### Canonicalization versions are durable identity boundaries

`scoped-spec-bundle/v2` is the default for new compilation and uses a v2
module-closure digest. Persisted bundle records carry a
`canonicalization_version` whose serde default is v1. Schema compilation
dispatches explicitly by the requested version.

The v1 canonicalizer and digest remain frozen. Existing v1 records are
readable, activatable, restart-restorable, and servable with their historical
bytes and identity. Retry or restoration of a v1 record never routes through
v2. New publication uses v2.

**Why this approach**: a canonicalization algorithm is part of a content
address, not an implementation detail that can change beneath persisted rows.

### Generated SDKs expose closed lifecycle types

`ManifestEntityV1` gains `lifecycle_states` with
`serde(default, skip_serializing_if = "Vec::is_empty")`. Empty legacy fields do
not change existing ABI-v1 manifest serialization or binding digests. Non-empty
states participate in the binding digest and `used_symbols` semantic hashes.

SDK generation consumes the canonical model. Enum-backed lifecycle properties
reuse their CSDL enum type. `Edm.String` lifecycle properties synthesize a
closed `<Entity>LifecycleState` serde string enum. Rust identifier collisions
fail generation with deterministic diagnostics rather than weakening the field
to an open string or renaming a wire value.

**Why this approach**: the generated API reflects the closed IOA state domain
without breaking unchanged v1 bindings.

### Registry activation atomically swaps rebuilt models

The registry stores the canonical model. Initial activation and hot reload
merge only stripped structural CSDL and the complete fully qualified IOA map,
rebuild one model, and atomically swap it after successful linking and
verification. Generated annotations are never used as merge inputs.

`$metadata`, schema pins, scoped deployment records, and restart restoration
serve the emitted CSDL owned by the activated model. Failed rebuilds leave the
previous model and metadata active and emit bounded activation-error telemetry.

**Why this approach**: behavior, transition tables, and public metadata change
as one revision, eliminating mixed-generation registry state.

## Rollout Plan

This decision ships as one architectural change rather than compatibility
scaffolding followed by deferred linking:

1. Add the versioned canonical linker, v1 freeze, v2 CSDL projection, and
   semantic contradiction diagnostics.
2. Move verification, JIT, SDK generation, bundle identity, activation,
   metadata serving, and restoration to the canonical model.
3. Migrate every maintained IOA to name its lifecycle property; remove authored
   behavioral projections from maintained CSDL; regenerate manifests, locks,
   SDK sources, schema pins, and checked-in WASM artifacts.
4. Exercise fresh activation, hot reload, action transitions, metadata,
   persisted v1 restart, and deployment telemetry before merge.

## Readiness Gates

- Linker tests cover lifecycle identity, state order, initial state, effective
  valid-from states, target states, parameter compatibility, action parity,
  enum dedication, namespace ambiguity, and mixed data-only entities.
- Deterministic fixtures prove formatting and input-order independence, legacy
  and annotation-free v2 convergence, v2 idempotence, frozen v1 digests, and
  stable v1 restart behavior.
- Verification and JIT parity tests prove they consume the same parsed
  automaton.
- Metadata tests cover mixed namespaces and generated `$metadata`.
- SDK golden and compile tests cover enum- and string-backed lifecycle states,
  manifest compatibility, serde wire values, and Rust identifier collisions.
- Full workspace tests, rustfmt, clippy, specification verification, artifact
  regeneration, mandatory DST review, and code-quality review pass.
- A local deployed app performs real transitions while serving the generated
  metadata; the deployed path confirms activation and error telemetry.

## Consequences

### Positive

- IOA becomes the single authored source of entity behavior.
- Every production consumer uses one immutable linked contract.
- Public OData metadata retains its existing state-machine surface.
- Equivalent v2 source representations converge on one identity.
- Data-only entities remain usable inside behavioral applications.

### Negative

- Bundle v2 publication is stricter and rejects latent CSDL/IOA disagreement.
- Lifecycle enum sharing is deliberately constrained.
- Changing canonicalization requires a new durable version and coordinated
  regeneration.
- The registry retains a larger immutable model to guarantee atomic parity.

### Risks

- Migration can expose legacy contradictions. Entity-qualified diagnostics and
  complete repository regeneration make each failure actionable.
- A missed production caller could retain a split input path. Call-site audits
  and parity tests gate removal of the old API.
- Regeneration can accidentally change external state names or ordinals.
  Golden metadata, SDK serde, and transition tests pin those values.
- Persisted v1 behavior could drift if shared helpers change. The v1
  canonicalizer and digest fixtures are frozen and exercised through restart.

### DST Compliance

- Canonical maps and sets use deterministic ordered collections; lifecycle and
  action state projections preserve IOA declaration order explicitly.
- Registry rebuild completes before one atomic revision swap. Actors never
  observe a partially linked model.
- No time, randomness, filesystem access, environment access, or concurrent
  task creation is added to simulation-visible linking or activation code.
- No new `// determinism-ok` suppression is anticipated.

## Non-Goals

- Removing state-machine annotations from the public OData metadata surface.
- Moving structural storage or OData concerns into IOA.
- Making unbound CSDL actions or functions executable IOA actions.
- Treating output actions as callable bound operations.
- Introducing another general-purpose entity specification language.
- Reinterpreting or silently upgrading persisted v1 bundle identity.

## Alternatives Considered

1. **Keep both authored forms and strengthen cross-validation** — Rejected
   because consumers would still have two inputs and formatting would still
   affect identity.
2. **Move the complete wire contract into IOA** — Rejected because it duplicates
   OData's structural type system and couples behavior to one transport.
3. **Make CSDL authoritative for lifecycle behavior** — Rejected because
   verification and execution already depend on IOA guards, effects, and
   invariants.
4. **Infer the lifecycle property forever** — Rejected because compatible
   properties can be ambiguous and runtime name heuristics are not an immutable
   contract.
5. **Rewrite v1 records through v2 on read** — Rejected because it changes the
   meaning of an existing content address and risks non-reproducible restart.

## Rollback Policy

Before v2 publication reaches a deployment, revert the implementation and
regenerated artifacts together. After v2 bundles are published, stop new v2
publication, reactivate the last compatible persisted v1 bundle, and roll back
the kernel. Never relabel v2 bytes as v1 or reinterpret a stored digest through
another canonicalizer. Persisted entity journals are unchanged because the
model changes linking and metadata projection, not event or state encoding.
