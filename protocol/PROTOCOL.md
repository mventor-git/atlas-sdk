# Atlas-SDK Plugin Protocol — v1.0.0

The plugin boundary is **this document**, not any language's object model.

A plugin in any language participates by speaking the messages below over its
own stdin/stdout, one JSON object per line. Nothing else crosses the boundary.

The Rust runtime's binding lives in `src/bridge.rs`. The reference binding in
Python lives in `plugins/py/pricing.py`. Neither is authoritative — this file is.

## Versioning

A plugin declares the protocol version it speaks in its manifest reply:

```json
{"type":"manifest","protocol":"1.0.0","manifest":{ ... }}
```

The runtime refuses any other version **before** it interprets the rest of the
manifest, with a deterministic error. Supported versions are listed in
`src/protocol.rs` (`SUPPORTED_PROTOCOL_VERSIONS`).

Adding an optional field is a minor version. Removing or retyping one is a major
version. A runtime must refuse a major version it does not implement rather than
guess.

## Manifest

The only thing the runtime knows about a plugin before it runs.

```json
{
  "id": "pricing",
  "version": "1.0.0",
  "contracts_provided": [{"id": "pricing.quote", "version": "1.0.0"}],
  "contracts_required": [{"id": "inventory.reserve", "version": "1.0.0"}],
  "subscriptions":       [{"id": "order.placed", "version": 1}],
  "capabilities":        [{"name": "quote:price", "requires": "pricing.quote"}],
  "lifecycle": {"kind": "ordered", "order": 20}
}
```

| Field | Meaning |
|---|---|
| `id` | Stable identity. Duplicates are refused. |
| `version` | The plugin's own version. |
| `contracts_provided` | Contracts this plugin implements, at an exact version. |
| `contracts_required` | Contracts this plugin needs from others, at an exact version. |
| `subscriptions` | Events this plugin reacts to. |
| `capabilities` | What it can do, and the authority each requires. |
| `lifecycle.kind` | `passive`, `ordered`, or `shutdown_aware`. |
| `lifecycle.order` | Initialize weight; lower runs first. Ignored unless `ordered`. |

A contract is satisfied only by an exact `id` + `version` match. A near version
is refused, never coerced.

## Exchange

### 1. Handshake

```json
-> {"type":"hello","protocol":"1.0.0"}
<- {"type":"manifest","protocol":"1.0.0","manifest":{ ... }}
```

A plugin that does not support the runtime's protocol version should answer
with `{"type":"error","error":"..."}` and exit non-zero.

### 2. Invoke a contract

The runtime sends this when a caller holding the declared authority resolves a
contract this plugin provides.

```json
-> {"type":"invoke","contract":"pricing.quote","version":"1.0.0",
    "principal":"ops@atlas","payload":{"quantity":3}}
<- {"type":"result","ok":true,"value":{"total":58.5}}
```

`principal` is the authority of the caller. **A plugin reads it; it never infers
it.** That is how a foreign plugin gets the same call-time authority guarantee
a native one has.

Failure:

```json
<- {"type":"result","ok":false,"error":"payload requires a numeric 'quantity'"}
```

### 3. Deliver an event

```json
-> {"type":"event","id":"order.placed","version":1,
    "principal":"ops@atlas","payload":{"order_id":"ORD-1001"}}
<- {"type":"ack","event":"order.placed","handled":"ORD-1001"}
```

An event is a fact that occurred. Delivery is the platform's job; interpreting
it is the plugin's. Subscribers are independent — no dependency is implied
between them.

### 4. Shutdown

```json
-> {"type":"ack","type":"shutdown"}
<- {"type":"ack","stopped":true}
```

The runtime sends this in reverse dependency order, then reaps the process.

## Errors

```json
{"type":"error","error":"unsupported message type 'frobnicate'"}
```

## Limits of v1.0.0

Stated plainly so nobody builds on an assumption that is not there:

- **No nested invocation.** A foreign plugin cannot call another contract
  mid-call. That needs a correlated reply protocol. Native plugins can, because
  their `Context` is in-process.
- **No streaming.** One request, one response.
- **No concurrency.** One plugin process serves requests serially.
- **Events are acknowledged, not returned.** A plugin cannot answer an event.
- **Errors are strings.** No error codes or typed failures across the boundary.

## Conformance

A plugin conforms if it satisfies `PROTOCOL.md` and passes the suite:

```sh
cargo run --bin atlas-conformance -- <command> [args...]
```

The suite runs the handshake, checks the manifest against this specification,
invokes each declared contract, delivers each declared event, and shuts down
cleanly. Exits non-zero on any violation.