//! The language-independent plugin protocol.
//!
//! This is the architectural boundary. Nothing here is a Rust type: messages
//! are JSON objects carried one per line over a plugin's stdin/stdout. A
//! binding in any language implements this and nothing more.
//!
//! The protocol is versioned. A plugin declares the version it speaks in its
//! manifest reply, and the runtime refuses a version it does not support —
//! deterministically, and before the plugin is started.

use crate::error::{SdkError, SdkResult};
use crate::identity::{Capability, ContractDecl, ContractId, Event, EventDecl, Version};
use crate::manifest::{Lifecycle, Manifest};
use crate::value::Value;

/// The protocol version this runtime speaks.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Every version this runtime can still accept. A plugin on any of these is
/// loaded; anything else is refused.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["1.0.0"];

/// Refuse a plugin whose declared protocol version is unsupported.
pub fn check_protocol_version(declared: &str) -> SdkResult<()> {
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&declared) {
        return Ok(());
    }
    Err(SdkError::UnsupportedProtocolVersion {
        declared: declared.to_string(),
        supported: SUPPORTED_PROTOCOL_VERSIONS.join(", "),
    })
}

// ---- message constructors -------------------------------------------------

/// Host -> plugin: opening handshake.
pub fn hello() -> Value {
    Value::map(vec![
        ("type", Value::str("hello")),
        ("protocol", Value::str(PROTOCOL_VERSION)),
    ])
}

/// Plugin -> host: the manifest, carrying the protocol version it speaks.
pub fn manifest_reply(manifest: &Manifest) -> Value {
    let provided: Vec<Value> = manifest
        .contracts_provided
        .iter()
        .map(|d| {
            Value::map(vec![
                ("id", Value::str(d.id.to_string())),
                ("version", Value::str(d.version.to_string())),
            ])
        })
        .collect();

    let required: Vec<Value> = manifest
        .contracts_required
        .iter()
        .map(|d| {
            Value::map(vec![
                ("id", Value::str(d.id.to_string())),
                ("version", Value::str(d.version.to_string())),
            ])
        })
        .collect();

    let capabilities: Vec<Value> = manifest
        .capabilities
        .iter()
        .map(|c| {
            Value::map(vec![
                ("name", Value::str(c.name.clone())),
                ("requires", Value::str(c.requires.clone())),
            ])
        })
        .collect();

    let subscriptions: Vec<Value> = manifest
        .subscriptions
        .iter()
        .map(|e| {
            Value::map(vec![
                ("id", Value::str(e.id.clone())),
                ("version", Value::Num(e.version as f64)),
            ])
        })
        .collect();

    let (kind, order) = match manifest.lifecycle {
        Lifecycle::Passive => ("passive", 0),
        Lifecycle::Ordered(n) => ("ordered", n),
        Lifecycle::ShutdownAware => ("shutdown_aware", 0),
    };

    Value::map(vec![
        ("type", Value::str("manifest")),
        ("protocol", Value::str(PROTOCOL_VERSION)),
        (
            "manifest",
            Value::map(vec![
                ("id", Value::str(manifest.id.clone())),
                ("version", Value::str(manifest.version.to_string())),
                ("contracts_provided", Value::List(provided)),
                ("contracts_required", Value::List(required)),
                ("subscriptions", Value::List(subscriptions)),
                ("capabilities", Value::List(capabilities)),
                (
                    "lifecycle",
                    Value::map(vec![
                        ("kind", Value::str(kind)),
                        ("order", Value::Num(order as f64)),
                    ]),
                ),
            ]),
        ),
    ])
}

/// Host -> plugin: invoke a contract the plugin declared.
pub fn invoke(contract: &ContractId, version: Version, principal: &str, payload: &Value) -> Value {
    Value::map(vec![
        ("type", Value::str("invoke")),
        ("contract", Value::str(contract.to_string())),
        ("version", Value::str(version.to_string())),
        ("principal", Value::str(principal)),
        ("payload", payload.clone()),
    ])
}

/// Plugin -> host: the result of an invocation.
pub fn result(ok: bool, value: Option<Value>, error: Option<String>) -> Value {
    let mut fields = vec![("type", Value::str("result")), ("ok", Value::Bool(ok))];
    if let Some(v) = value {
        fields.push(("value", v));
    }
    if let Some(e) = error {
        fields.push(("error", Value::str(e)));
    }
    Value::map(fields)
}

/// Host -> plugin: deliver an event the plugin subscribed to.
pub fn event(event: &Event, principal: &str) -> Value {
    Value::map(vec![
        ("type", Value::str("event")),
        ("id", Value::str(event.id.clone())),
        ("version", Value::Num(event.version as f64)),
        ("principal", Value::str(principal)),
        ("payload", event.payload.clone()),
    ])
}

/// Host -> plugin: an acknowledgement with nothing to carry.
pub fn ack(kind: &str) -> Value {
    Value::map(vec![("type", Value::str(kind))])
}

// ---- message readers -----------------------------------------------------

fn field<'a>(message: &'a Value, key: &str) -> SdkResult<&'a Value> {
    message.get(key).ok_or_else(|| SdkError::ProtocolViolation {
        detail: format!("missing field '{key}'"),
    })
}

fn field_str<'a>(message: &'a Value, key: &str) -> SdkResult<&'a str> {
    field(message, key)?
        .as_str()
        .ok_or_else(|| SdkError::ProtocolViolation {
            detail: format!("field '{key}' must be a string"),
        })
}

fn message_type(message: &Value) -> SdkResult<&str> {
    field_str(message, "type")
}

/// Parse a `version` string such as `1.2.3`.
pub fn parse_version(text: &str) -> SdkResult<Version> {
    let mut parts = text.split('.');
    let mut next = |what: &str| -> SdkResult<u32> {
        parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .ok_or_else(|| SdkError::ProtocolViolation {
                detail: format!("malformed version '{text}' (expected {what})"),
            })
    };
    Ok(Version::new(
        next("major.minor.patch")?,
        next("major.minor.patch")?,
        next("major.minor.patch")?,
    ))
}

/// Read a manifest reply into a `Manifest`, refusing an unsupported protocol
/// version before anything else is interpreted.
pub fn read_manifest(message: &Value) -> SdkResult<Manifest> {
    if message_type(message)? != "manifest" {
        return Err(SdkError::ProtocolViolation {
            detail: format!(
                "expected a manifest reply, got '{}'",
                message_type(message)?
            ),
        });
    }

    let declared = field_str(message, "protocol")?;
    check_protocol_version(declared)?;

    let m = field(message, "manifest")?;

    let contracts = |key: &str| -> SdkResult<Vec<ContractDecl>> {
        let list = field(m, key)?
            .as_list()
            .ok_or_else(|| SdkError::ProtocolViolation {
                detail: format!("'{key}' must be a list"),
            })?;
        list.iter()
            .map(|entry| {
                Ok(ContractDecl {
                    id: ContractId::new(field_str(entry, "id")?),
                    version: parse_version(field_str(entry, "version")?)?,
                })
            })
            .collect()
    };

    let mut capabilities = Vec::new();
    for entry in field(m, "capabilities")?
        .as_list()
        .ok_or_else(|| SdkError::ProtocolViolation {
            detail: "\"capabilities\" must be a list".into(),
        })?
    {
        capabilities.push(Capability::new(
            field_str(entry, "name")?,
            field_str(entry, "requires")?,
        ));
    }

    let mut subscriptions = Vec::new();
    for entry in
        field(m, "subscriptions")?
            .as_list()
            .ok_or_else(|| SdkError::ProtocolViolation {
                detail: "\"subscriptions\" must be a list".into(),
            })?
    {
        subscriptions.push(EventDecl {
            id: field_str(entry, "id")?.to_string(),
            version: field(entry, "version")?.as_num().unwrap_or(0.0) as u32,
        });
    }

    let lifecycle_field = field(m, "lifecycle")?;
    let lifecycle = match field_str(lifecycle_field, "kind")? {
        "passive" => Lifecycle::Passive,
        "ordered" => Lifecycle::Ordered(
            lifecycle_field
                .get("order")
                .and_then(Value::as_num)
                .unwrap_or(0.0) as i32,
        ),
        "shutdown_aware" => Lifecycle::ShutdownAware,
        other => {
            return Err(SdkError::ProtocolViolation {
                detail: format!("unknown lifecycle kind '{other}'"),
            })
        }
    };

    Ok(Manifest {
        id: field_str(m, "id")?.to_string(),
        version: parse_version(field_str(m, "version")?)?,
        contracts_provided: contracts("contracts_provided")?,
        contracts_required: contracts("contracts_required")?,
        subscriptions,
        capabilities,
        lifecycle,
    })
}

/// Read a plugin's reply to an invocation.
pub fn read_result(message: &Value) -> SdkResult<SdkResult<Value>> {
    if message_type(message)? != "result" {
        return Err(SdkError::ProtocolViolation {
            detail: format!("expected a result, got '{}'", message_type(message)?),
        });
    }
    match field(message, "ok")? {
        Value::Bool(true) => Ok(Ok(field(message, "value")?.clone())),
        Value::Bool(false) => Ok(Err(SdkError::HandlerFailed {
            plugin: "foreign".into(),
            reason: field_str(message, "error")?.to_string(),
        })),
        _ => Err(SdkError::ProtocolViolation {
            detail: "'ok' must be a boolean".into(),
        }),
    }
}

/// Read an event message.
pub fn read_event(message: &Value) -> SdkResult<Event> {
    if message_type(message)? != "event" {
        return Err(SdkError::ProtocolViolation {
            detail: format!("expected an event, got '{}'", message_type(message)?),
        });
    }
    Ok(Event {
        id: field_str(message, "id")?.to_string(),
        version: field(message, "version")?.as_num().unwrap_or(0.0) as u32,
        payload: field(message, "payload")?.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest::new("pricing", Version::new(1, 0, 0))
            .provides(ContractDecl::new("pricing.quote", Version::new(2, 1, 0)))
            .requires(ContractDecl::new(
                "inventory.reserve",
                Version::new(1, 0, 0),
            ))
            .subscribes(EventDecl::new("order.placed", 1))
            .capability(Capability::new("quote:price", "pricing.quote"))
            .lifecycle(Lifecycle::Ordered(5))
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let original = sample_manifest();
        let wire = crate::json::to_json(&manifest_reply(&original));
        let decoded = crate::json::parse_json(&wire).unwrap();
        assert_eq!(read_manifest(&decoded).unwrap(), original);
    }

    #[test]
    fn accepts_a_supported_protocol_version() {
        assert!(check_protocol_version("1.0.0").is_ok());
    }

    #[test]
    fn refuses_an_unsupported_protocol_version_deterministically() {
        let err = check_protocol_version("2.0.0").unwrap_err();
        assert_eq!(
            err,
            SdkError::UnsupportedProtocolVersion {
                declared: "2.0.0".into(),
                supported: "1.0.0".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "plugin speaks protocol version 2.0.0 which this runtime does not support (supported: 1.0.0)"
        );
    }

    #[test]
    fn refuses_a_plugin_before_parsing_the_rest_of_its_manifest() {
        // Version is checked first, so a malformed body never even gets read.
        let wire = crate::json::to_json(&manifest_reply(&sample_manifest()))
            .replace("\"protocol\":\"1.0.0\"", "\"protocol\":\"9.9.9\"");
        let reply = crate::json::parse_json(&wire).unwrap();
        let err = read_manifest(&reply).unwrap_err();
        assert!(matches!(err, SdkError::UnsupportedProtocolVersion { .. }));
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("1.2.3").unwrap(), Version::new(1, 2, 3));
        assert!(parse_version("1.2").is_err());
        assert!(parse_version("x.y.z").is_err());
    }

    #[test]
    fn reads_results_both_ways() {
        let ok = result(true, Some(Value::num(7)), None);
        assert_eq!(read_result(&ok).unwrap().unwrap(), Value::num(7.0));

        let bad = result(false, None, Some("boom".into()));
        assert!(read_result(&bad).unwrap().is_err());
    }

    #[test]
    fn rejects_a_message_of_the_wrong_type() {
        assert!(read_manifest(&ack("nope")).is_err());
        assert!(read_result(&ack("nope")).is_err());
        assert!(read_event(&ack("nope")).is_err());
    }

    #[test]
    fn rejects_a_manifest_missing_a_required_field() {
        let mut reply = manifest_reply(&sample_manifest());
        if let Value::Map(m) = &mut reply {
            m.remove("protocol");
        }
        assert!(matches!(
            read_manifest(&reply),
            Err(SdkError::ProtocolViolation { .. })
        ));
    }
}
