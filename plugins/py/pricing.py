#!/usr/bin/env python3
"""Reference binding for the Atlas-SDK plugin protocol.

This file is the second-language proof. It speaks the protocol documented in
`protocol/PROTOCOL.md` using nothing but the Python standard library, and
knows nothing about the Rust runtime, its types, or its process.

It demonstrates the whole contract boundary from outside:

  * it declares a manifest in the wire format, including a protocol version
  * it serves a contract, and reports the calling principal back
  * it consumes an event it subscribed to

Run it standalone to see the handshake:

    python plugins/py/pricing.py
"""

import json
import sys

PROTOCOL_VERSION = "1.0.0"

# What this plugin is and what it offers. Declared as plain data, which is the
# entire point: the runtime reads this shape, not a Python class.
MANIFEST = {
    "id": "pricing",
    "version": "1.0.0",
    "contracts_provided": [{"id": "pricing.quote", "version": "1.0.0"}],
    "contracts_required": [{"id": "inventory.reserve", "version": "1.0.0"}],
    "subscriptions": [{"id": "order.placed", "version": 1}],
    "capabilities": [{"name": "quote:price", "requires": "pricing.quote"}],
    "lifecycle": {"kind": "ordered", "order": 20},
}

# Business state. A real plugin would have something; this one just counts.
_QUOTES_ISSUED = 0


def _send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def _manifest_reply():
    return {
        "type": "manifest",
        "protocol": PROTOCOL_VERSION,
        "manifest": MANIFEST,
    }


def _handle_invoke(request):
    """Serve the contract. `principal` is supplied by the runtime, never guessed."""
    global _QUOTES_ISSUED

    contract = request.get("contract")
    principal = request.get("principal", "unknown")
    payload = request.get("payload") or {}

    if contract != "pricing.quote":
        return {
            "type": "result",
            "ok": False,
            "error": f"unknown contract '{contract}'",
        }

    quantity = payload.get("quantity")
    if not isinstance(quantity, (int, float)):
        return {
            "type": "result",
            "ok": False,
            "error": "payload requires a numeric 'quantity'",
        }

    _QUOTES_ISSUED += int(quantity)

    return {
        "type": "result",
        "ok": True,
        "value": {
            "quantity": quantity,
            "unit_price": 19.5,
            "total": round(quantity * 19.5, 2),
            "currency": "EGP",
            "served_by": "pricing",
            "principal": principal,
        },
    }


def _handle_event(event):
    """A fact occurred. Meaning is this plugin's business, not the SDK's."""
    order_id = (event.get("payload") or {}).get("order_id", "unknown")
    _send(
        {
            "type": "ack",
            "event": event.get("id"),
            "handled": order_id,
        }
    )
    return True


def main():
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue

        try:
            message = json.loads(raw)
        except json.JSONDecodeError as exc:
            _send({"type": "error", "error": f"malformed JSON: {exc}"})
            return 1

        kind = message.get("type")

        if kind == "hello":
            # Refuse politely if the runtime speaks something we do not.
            if message.get("protocol") != PROTOCOL_VERSION:
                _send(
                    {
                        "type": "error",
                        "error": "unsupported protocol version "
                        f"{message.get('protocol')}",
                    }
                )
                return 1
            _send(_manifest_reply())

        elif kind == "invoke":
            _send(_handle_invoke(message))

        elif kind == "event":
            _handle_event(message)

        elif kind == "shutdown":
            _send({"type": "ack", "stopped": True})
            return 0

        else:
            _send({"type": "error", "error": f"unsupported message type '{kind}'"})

    return 0


if __name__ == "__main__":
    sys.exit(main())