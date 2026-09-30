# Atlas-SDK

A general-purpose application runtime and plugin platform.

The SDK owns the orchestration lifecycle. A host supplies environment and
plugins; it does not supply a competing lifecycle, registry or contract system.

This crate currently proves exactly one thing — the fundamental loop:

```text
discover -> validate -> register -> initialize -> expose capability
  -> resolve contract -> publish/consume event -> execute with context
  -> shutdown
```

Everything else is deliberately absent until a real requirement demands it.

## Run it

```sh
cargo run      # the demo host, printing all nine steps in order
cargo test     # 25 tests, one per acceptance criterion
cargo clippy --all-targets -- -D warnings
```

Requires a stable Rust toolchain (built and verified on 1.98.1). The host
triple `x86_64-pc-windows-gnu` is used on this machine because it ships its own
linker; nothing in the code depends on the triple.

## What is proven

| Criterion | Where |
|---|---|
| Full loop, in order, one process | `src/main.rs`, `criterion_01_fundamental_loop_runs_end_to_end` |
| Two plugins, fully declared manifests | `criterion_02_both_manifests_are_fully_declared` |
| Zero direct plugin-to-plugin coupling | `criterion_03_plugins_do_not_import_each_other` |
| Duplicate identity refused, never started | `criterion_04_duplicate_identity_is_refused_and_never_started` |
| Platform services via SDK context ports | `criterion_05_plugin_reads_platform_services_through_the_port` |
| Authority visible to the callee at call time | `criterion_06_authority_is_visible_to_the_callee_at_call_time` |
| Reverse-order, idempotent shutdown | `criterion_07_shutdown_runs_in_reverse_dependency_order`, `..._is_idempotent` |

## How the guarantees are enforced

**No plugin-to-plugin coupling.** A plugin receives exactly one thing: a
`Context`. The context exposes platform ports — identity and authority, audit,
log, config, contracts, events. It exposes no handle to another plugin, to the
registry, or to the host, so coupling is not something a plugin can express.
The two plugins live in separate files and neither may import the other; a test
scans both for a `use` line naming the other's module and fails if one appears.

The consumer (`orders`) and the provider (`inventory`) declare the contract
identity `"inventory.reserve"` **independently**, as strings. They agree by
name, not by sharing a symbol — a shared constant would have made the
guarantee worthless.

**Authority.** `Authority { principal, granted }` is constructed at the call
site and carried inside the `Context`. A callee reads `ctx.principal()` during
the call and reports it back in its response; nothing consults a global or a
thread-local. Authority propagates across a contract hop rather than resetting.
Before invoking a contract the runtime compares the caller against the
`requires` field the provider declared in its manifest.

**Determinism.** Every refusal is a typed `SdkError` with a stable rendered
message, asserted by string in the tests. Shutdown order is by dependency
depth descending with discovery order as tie-break, so independent plugins
keep the order they arrived in.

**Version compatibility.** A required contract is satisfied only by an exact
identity+version match. A mismatch is refused with
`UnsupportedContractVersion`; it is never coerced.

## Why no dependencies

`std` only — no async, no serde, no framework. Payloads use a small hand-rolled
`Value` enum. This keeps the proof about the architecture rather than about a
type system or a runtime, and it means the crate builds offline.

## Layout

```text
src/
  error.rs       typed, deterministic refusals
  value.rs       minimal payload type
  identity.rs    Version, ContractId, Capability, Authority, Event
  manifest.rs    what a plugin declares
  context.rs     the SDK-defined context port (the only door a plugin has)
  plugin.rs      the Plugin trait
  runtime.rs     the registry and the lifecycle
  plugins/       one file per plugin, on purpose
    inventory.rs   provides inventory.reserve
    orders.rs      requires it, subscribes to order.placed
tests/
  fundamental_loop.rs
```

## Known limits

- Single-threaded. The shared platform is `Rc<RefCell<..>>`; concurrency would
  need `Arc<Mutex<..>>`. Marked `ponytail` in `context.rs`.
- Event delivery is in-process only. No durable outbox, no retries, no
  cross-host transport. All of that is deferred by contract, not by oversight.
- `Value` cannot represent every JSON document. `serde_json` is the upgrade
  path if that becomes a requirement.
- The protocol is not yet language-independent — that is contract item C-002.

## Contract

`.contract/` is the project's governing vision and execution state. It is
written only through the contract MCP. **Do not hand-edit it, and do not add a
`contract.md`** — a parallel scope file is an explicit project non-goal.