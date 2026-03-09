# Spec 10: rtt-core Cleanup and Test Organization

## Priority: SHOULD HAVE (maintainability, clearer boundaries, safer iteration)

## Recommended Order

Run this spec before [specs/09-rtt-core-refactor.md](/Users/sam/Desktop/Projects/rtt/specs/09-rtt-core-refactor.md).

Reason:

- it separates offline and live verification more cleanly
- it removes misleading or dead surfaces before optimization work begins
- it reduces the chance that latency work optimizes the wrong path

Constraint:

- keep cleanup surgical
- if a cleanup change materially alters hot-path behavior, capture baseline and after-change measurements using the verification commands from Spec 09

## Problem

`rtt-core` has accumulated a mix of production code, benchmark support, dead or misleading APIs, and live-network tests embedded in library modules. None of that is the primary speed bottleneck, but it does slow down implementation work and makes it easier to misread what is actually part of the hot path.

This work is intentionally separate from Spec 09.

- Spec 09 is for measurable latency improvements and live no-regression proof.
- Spec 10 is for cleanup, clearer module boundaries, and better test layout.

If a proposed cleanup change risks the hot path, it belongs under Spec 09 with benchmark proof, not here.

## Current Code

- `crates/rtt-core/src/clob_request.rs`
  - exposes `find_salt_position()` and `build_order_template()` even though mutating signed payloads is misleading for EIP-712-signed orders
- `crates/rtt-core/src/clob_executor.rs`
  - contains `ClobExecutionConfig`, which appears unused outside tests / historical flow
- `crates/rtt-core/src/connection.rs`, `crates/rtt-core/src/executor.rs`, `crates/rtt-core/src/benchmark.rs`, `crates/rtt-core/src/h3_stub.rs`
  - include live-network tests in `#[cfg(test)]` blocks under `src/*`
- `crates/rtt-core/README.md`
  - does not cleanly separate offline tests from live integration tests
- `crates/rtt-core/src/*`
  - several APIs return broad `Box<dyn Error>` or encode different failure classes loosely, which is workable but not very legible

## Solution

### Big Task 1: Remove dead or misleading APIs

#### Sub-task 1.1: Remove signed-payload salt-patching helpers or mark them internal-only

`find_salt_position()` and `build_order_template()` should not remain public in a form that implies signed order bodies can be safely mutated after signing.

Options:

- remove them entirely if unused
- make them private/internal if only needed for tests or experiments
- rewrite their docs so they clearly do not apply to signed CLOB orders

#### Sub-task 1.2: Delete dead config and stale compatibility layers

Evaluate and remove:

- `ClobExecutionConfig` if it is unused
- stale request-template comments that no longer describe the real live path
- compatibility code that remains only because earlier pre-sign flows existed

### Big Task 2: Split offline and live tests cleanly

#### Sub-task 2.1: Move live-network tests out of `src/*`

Network-dependent tests in these modules should move to `crates/rtt-core/tests/` or a similar integration-test location:

- `connection.rs`
- `executor.rs`
- `benchmark.rs`
- `h3_stub.rs`

Goal:

- `cargo test -p rtt-core --lib` becomes truly offline
- live-network coverage remains available through explicit integration-test commands

#### Sub-task 2.2: Keep the live commands stable and documented

The cleanup must not hide how to run real checks.

Required command categories:

- offline unit tests
- live integration tests
- benchmark command
- reject-path `fire.sh` command
- acceptance-path `fire.sh` command

These commands should match the verification section in Spec 09.

### Big Task 3: Improve codebase legibility without changing the execution model

#### Sub-task 3.1: Narrow comments to actual behavior

Remove or update comments that overstate guarantees, especially around:

- “zero allocation” claims that no longer hold
- request-template mutation assumptions
- benchmark vs production-path descriptions

#### Sub-task 3.2: Introduce clearer internal failure names where they help reviewability

If the implementation under Spec 09 does not already do this, cleanup can introduce narrower internal error naming or helper types where that materially improves code review and maintenance.

This is not a mandate to create a large error framework. Keep it small.

### Big Task 4: Refresh docs around how to work in `rtt-core`

#### Sub-task 4.1: Update crate README test instructions

[README.md](/Users/sam/Desktop/Projects/rtt/crates/rtt-core/README.md) should clearly separate:

- offline tests
- live integration tests
- ignored real-order tests

#### Sub-task 4.2: Cross-reference the latency spec

The cleanup spec should explicitly point readers to Spec 09 for any hot-path or benchmark-sensitive change.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/rtt-core/src/clob_request.rs` | Remove or de-publicize misleading salt-patching APIs |
| `crates/rtt-core/src/clob_executor.rs` | Remove unused config / stale compatibility surfaces |
| `crates/rtt-core/src/request.rs` | Align comments and API claims with actual usage |
| `crates/rtt-core/src/connection.rs` | Move live tests out of `src/*` if not done under Spec 09 |
| `crates/rtt-core/src/executor.rs` | Move live tests out of `src/*` if not done under Spec 09 |
| `crates/rtt-core/src/benchmark.rs` | Move live tests out of `src/*` if not done under Spec 09 |
| `crates/rtt-core/src/h3_stub.rs` | Move live tests out of `src/*` if not done under Spec 09 |
| `crates/rtt-core/tests/*` | Receive moved live integration tests |
| `crates/rtt-core/README.md` | Clarify test commands and categories |
| `specs/09-rtt-core-refactor.md` | Cross-reference if needed |

## Tests

1. `cargo test -p rtt-core --lib` runs without hitting the network
2. `cargo test -p rtt-core --test '*'` still runs the live integration suite
3. The documented verification commands remain valid after test movement
4. Any removed API has either no remaining callers or updated callers with equivalent behavior

## Acceptance Criteria

- [ ] `cargo test -p rtt-core --lib` is fully offline
- [ ] Live integration coverage still exists and is easy to run
- [ ] Misleading signed-payload mutation APIs are removed, hidden, or explicitly documented as non-production
- [ ] Unused config / stale compatibility surfaces are removed
- [ ] `crates/rtt-core/README.md` clearly explains offline vs live vs real-order tests
- [ ] Spec 09 remains the source of truth for speed-sensitive verification

## Scope Boundaries

- Do NOT claim latency wins from this spec without benchmark proof from Spec 09
- Do NOT rewrite the dedicated execution model
- Do NOT change Polymarket wire semantics
- Do NOT replace working code with larger abstractions just for style
