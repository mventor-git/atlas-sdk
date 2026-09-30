//! The conformance suite.
//!
//! A plugin in any language conforms if it speaks `protocol/PROTOCOL.md` and
//! passes this. The suite drives the plugin as a black box over the wire: it
//! reads nothing from the plugin's source and imports nothing from its
//! language, because the point is that neither should matter.
//!
//! Usage:
//!   cargo run --bin atlas-conformance -- python plugins/py/pricing.py
//!
//! Prints every violation and exits non-zero if the plugin does not conform.

use std::process::Command;
use std::time::Duration;

use atlas_sdk::bridge::ForeignPlugin;
use atlas_sdk::identity::Event;
use atlas_sdk::protocol;
use atlas_sdk::value::Value;
use atlas_sdk::Plugin;

/// How long a plugin may take to answer before it is judged non-conformant.
/// Short on purpose: a suite that hangs is worse than a suite that fails.
const SUITE_TIMEOUT: Duration = Duration::from_secs(3);

/// The principal the suite presents. A plugin must report this back verbatim,
/// which is what proves it reads call-time authority rather than assuming one.
const PROBE_PRINCIPAL: &str = "conformance@atlas";

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn conform(command: &str, args: &[String]) -> Result<String, String> {
    let mut parts = command.split_whitespace();
    let program = parts.next().ok_or("no plugin command given")?;
    let mut argv: Vec<String> = parts.map(str::to_string).collect();
    argv.extend_from_slice(args);

    Command::new(program)
        .arg("--nonexistent-conformance-probe")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("cannot execute '{program}': {e}"))?;

    // Connecting performs the handshake and enforces the protocol version.
    let mut plugin = ForeignPlugin::connect_with_timeout(program, argv, SUITE_TIMEOUT)
        .map_err(|e| e.to_string())?;
    let manifest = plugin.manifest().clone();

    check(!manifest.id.trim().is_empty(), "manifest id is empty")?;
    check(
        manifest.validate().is_ok(),
        &format!(
            "manifest is structurally invalid: {}",
            manifest.validate().err().unwrap_or_default()
        ),
    )?;
    check(
        !manifest.contracts_provided.is_empty() || !manifest.contracts_required.is_empty(),
        "plugin declares no contracts at all",
    )?;

    // Every contract the plugin claims to provide must actually answer, and must
    // report the caller rather than a value it invented.
    for decl in &manifest.contracts_provided {
        let response = plugin
            .invoke_contract(
                &decl.id,
                decl.version,
                PROBE_PRINCIPAL,
                Value::map(vec![("quantity", Value::num(2))]),
            )
            .map_err(|e| format!("contract {} failed: {e}", decl.id))?;

        check(
            response.get("principal").and_then(Value::as_str) == Some(PROBE_PRINCIPAL),
            &format!(
                "contract {} did not report its caller (expected {PROBE_PRINCIPAL}, got {:?})",
                decl.id,
                response.get("principal")
            ),
        )?;
    }

    // Every event the plugin claims to subscribe to must be accepted.
    for event in &manifest.subscriptions {
        plugin
            .deliver_event(
                &Event::new(
                    event.id.clone(),
                    event.version,
                    Value::map(vec![("order_id", Value::str("CONFORMANCE"))]),
                ),
                PROBE_PRINCIPAL,
            )
            .map_err(|e| format!("event {} was refused: {e}", event.id))?;
    }

    plugin.shutdown().map_err(|e| e.to_string())?;
    check(plugin.stopped(), "plugin did not acknowledge shutdown")?;

    Ok(format!(
        "{} v{} — {} contract(s), {} event(s)",
        manifest.id,
        manifest.version,
        manifest.contracts_provided.len(),
        manifest.subscriptions.len()
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: atlas-conformance <command> [args...]");
        eprintln!("example: atlas-conformance python plugins/py/pricing.py");
        std::process::exit(2);
    }

    match conform(&args[0], &args[1..]) {
        Ok(summary) => {
            println!("CONFORMANT      protocol v{}", protocol::PROTOCOL_VERSION);
            println!("  {summary}");
        }
        Err(message) => {
            eprintln!("NOT CONFORMANT  protocol v{}", protocol::PROTOCOL_VERSION);
            eprintln!("  {message}");
            std::process::exit(1);
        }
    }
}
