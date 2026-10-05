# Tanuki implementation instructions

## Authority and scope

Read `docs/spec.md` for product behaviour, this file for engineering
requirements, `docs/implementation-plan.md` for task order, `docs/design.md`
for proposed local interfaces, and `docs/test-plan.md` for acceptance
scenarios. `docs/decisions-to-review.md` contains proposals as well as accepted
decisions: do not silently promote proposals into requirements. Numbered notes
are historical only. Later user instructions take precedence.

Implement the current task, not every future feature. Keep Tanuki simple,
permissive where practical, and fun to hack on. Log unusual behaviour
liberally. Do not add replay, distributed consensus, a plugin framework, or a
general permissions system to solve hypothetical edge cases.

## Rust and nightly

Use Rust nightly. Pin a working dated nightly in rust-toolchain.toml, including
rustfmt and Clippy, and commit Cargo.lock for the application. Choose and
verify the actual toolchain during implementation; no date is prescribed here.
Nightly upgrades should be deliberate, not an attempt to support stable by
default.

Before writing a cumbersome workaround, long combinator chain, helper
abstraction, or adding a compatibility dependency, check current
standard-library documentation, the Unstable Book, and the selected crate's API
docs. Prefer a clear native API, including an unstable API when appropriate. Do
not adopt unstable features merely for novelty. Confirm syntax and feature
gates with a small compile check against the pinned compiler; documentation for
latest nightly may differ from the pin.

Prefer native async traits over async-trait. Where returned futures need Send
bounds, check current language support rather than reflexively boxing or adding
a macro crate. Dyn compatibility is a separate requirement: introduce dynamic
dispatch only where useful, then choose the smallest justified boundary. Do not
claim every async trait needs an unstable feature or every future needs Send.

Record enabled unstable features, their purpose, and documentation links in
docs/toolchain.md. Remove obsolete gates/workarounds when upgrading. Stable
compatibility is not an initial requirement.

## Domain modelling

Use distinct types for distinct concepts: topic paths, selectors, client names,
session IDs, claims, timestamps, deadlines, event occurrences, and retained
payloads. Prefer semantic enums over booleans where they communicate intent or
eliminate unsupported combinations. If only three node kinds existed, represent
three variants; do not allow a fourth combination and rely on constructors to
reject it. Currently all four kinds are supported.

Make internal invalid states unrepresentable in the representation where
practical; otherwise enforce invariants at the owning type's API boundary. Keep
invariant-bearing fields private. Avoid duplicating facts such as a stored kind
flag and an unrelated payload enum that can disagree. Deserialize external DTOs
and validate conversions; do not derive a route around invariant-enforcing
constructors.

Schema correctness is a higher-level policy, not a Rust representation
invariant. A structurally valid Value remains representable whether it passes,
warns, or fails a schema. Do not encode every user schema into Rust types or
confuse warning acceptance with an internal bug.

Use custom error enums, preferably derived with thiserror. Do not use anyhow.
Preserve error sources and meaningful variants; map errors at subsystem
boundaries without reducing everything to strings. Warnings are
successful-operation diagnostics, not fabricated errors.

## Validation and assertions

Choose one primary validation site on the mutation path. Every public write
route, timer mutation, schema update, and restore operation must follow its
documented applicable checks; adapters must not mutate storage independently.

Use side-effect-free debug assertions to double-check assumptions at convenient
downstream boundaries. A schema assertion is appropriate only if it can cheaply
inspect the same policy/candidate context. Assert absence of denying
violations, not absence of warnings. It must not cast again, log again, update
metadata, or compare against an unrelated newer schema revision.

Debug assertions are supplementary developer checks. Release correctness must
not depend on them. Do not change the architecture or retain large validation
state just to add a redundant check. Unexpected user input gets a typed error
or warning, not an assertion panic.

## Implementation conventions

Use Axum for HTTP/WebSocket unless a concrete issue warrants discussing an
alternative. Define common typed response and error conversion machinery;
handlers must not construct inconsistent ad hoc JSON. Use request extractors
for recurring parsing/context and middleware for cross-cutting concerns.
Extractor rejections must use the common error format too.

Prefer established ecosystem crates where they remove real work: Tokio, Serde,
thiserror, Axum, tracing, Jiff and suitable JSON/MessagePack libraries are the
expected starting set. Check APIs and select versions during implementation. Do
not add a crate for something clearly expressed by the standard library, and do
not reimplement a substantial supported facility just to avoid dependencies.

Start with a library and a thin server binary in a modest module tree. Keep
domain logic independent of HTTP, sockets and disk. Avoid a trait, service, or
workspace crate per tiny function. Introduce seams for time, transport and
persistence because the tests actually need them.

## TDD and quality checks

For each meaningful behaviour: specify the scenario, write a test that fails
for the intended missing behaviour, implement the minimum coherent solution,
then refactor. A missing import alone is not evidence of a useful red phase;
establish the behavioural failure when the API exists. Do not weaken a test to
fit an implementation without identifying the changed requirement.

Maintain unit, integration, and end-to-end tests. Whole-system tests use actual
server logic and transport with simulated external producers/consumers; do not
mock the server behaviour being verified. See `docs/test-plan.md`.

Run rustfmt, Clippy, and relevant tests for each task. At milestone boundaries
run the full supported suite and release-mode correctness checks. Do not
silence broad lint classes to make a gate green. Targeted allowances require a
short reason. Use property tests for combinatorial rules where valuable, not as
a substitute for named examples. Introduce heavier tools only for an actual
risk.

## User involvement and documentation

Keep the user involved at the architecture checkpoints in
`docs/implementation-plan.md`. Present short snippets of types and local APIs, how
data flows through them, and one real tradeoff. Ask for feedback on
consequential choices, not every helper signature. Do not repeatedly ask
permission to continue already agreed work. Routine reversible choices can
proceed with a documented default.

Maintain docs/architecture.md, docs/protocol.md, docs/procedures.md, and
docs/toolchain.md as implementation evolves. Document ownership of state,
mutation/validation/publication order, startup/recovery, shutdown, disconnect
and expiry. Document why modules exist and why non-obvious policies were
selected.

For task completion report behaviour delivered, tests run and their results,
any chosen provisional defaults, and remaining limitations. Do not mark an
entire milestone done because only its happy path works.
