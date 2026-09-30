//! Acceptance tests for contract item C-002.
//!
//! The claim under test is that the plugin boundary is a protocol rather than a
//! Rust object model. The strongest evidence is not that it works once, but that
//! it works through the *same* runtime, registry, context port and authority
//! checks as a native plugin, with no special-casing anywhere.

use std::path::PathBuf;

use atlas_sdk::error::SdkError;
use atlas_sdk::identity::{Authority, ContractId, Event, Version};
use atlas_sdk::json;
use atlas_sdk::protocol;
use atlas_sdk::value::Value;
use atlas_sdk::{Context, ForeignPlugin, Manifest, Runtime, SUPPORTED_PROTOCOL_VERSIONS};

/// Where the Python reference binding lives.
fn pricing_plugin() -> String {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("plugins")
        .join("py")
        .join("pricing.py")
        .to_string_lossy()
        .into_owned()
}

fn foreign() -> Box<dyn atlas_sdk::Plugin> {
    Box::new(
        ForeignPlugin::connect("python", vec![pricing_plugin()])
            .expect("the reference binding must complete the handshake"),
    )
}

fn ops() -> Authority {
    Authority::new("ops@atlas", vec!["pricing.quote".into()])
}

// ---- Criterion 1: the protocol is a schema, not Rust types ---------------

/// Criterion 1. The wire format is JSON with the field names the specification
/// publishes, so a plugin author in any language can build a manifest by hand.
#[test]
fn criterion_01_manifest_is_a_language_neutral_schema() {
    let manifest = Manifest::new("pricing", Version::new(1, 0, 0))
        .provides(atlas_sdk::ContractDecl::new(
            "pricing.quote",
            Version::new(1, 0, 0),
        ))
        .subscribes(atlas_sdk::EventDecl::new("order.placed", 1));

    let wire = json::to_json(&protocol::manifest_reply(&manifest));

    // Field names and JSON shapes, asserted literally. No Rust type appears.
    assert!(wire.contains(r#""contracts_provided":[{"id":"pricing.quote","version":"1.0.0"}]"#));
    assert!(wire.contains(r#""subscriptions":[{"id":"order.placed","version":1}]"#));
    assert!(wire.contains(r#""protocol":"1.0.0""#));
    assert!(wire.contains(r#""type":"manifest""#));

    // And it round-trips back into the same manifest.
    let decoded = protocol::read_manifest(&json::parse_json(&wire).unwrap()).unwrap();
    assert_eq!(decoded, manifest);
}

// ---- Criterion 2: the protocol is versioned and refusals are hard --------

/// Criterion 2. A plugin on an unsupported protocol version is refused with a
/// deterministic error, before any of its manifest is interpreted.
#[test]
fn criterion_02_unsupported_protocol_version_is_refused_deterministically() {
    assert_eq!(SUPPORTED_PROTOCOL_VERSIONS, &["1.0.0"]);

    let err = protocol::check_protocol_version("2.0.0").unwrap_err();
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

/// Criterion 2, stronger. The refusal happens against a real foreign process,
/// not just a string.
#[test]
fn criterion_02_foreign_plugin_on_a_bad_version_is_refused_before_registration() {
    // A plugin that answers hello with a version nobody supports.
    let script = dir().join("bad_version.py");
    std::fs::write(
        &script,
        r#"import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m["type"] == "hello":
        print(json.dumps({"type": "manifest", "protocol": "7.7.7",
                          "manifest": {"id": "x", "version": "1.0.0",
                                       "contracts_provided": [], "contracts_required": [],
                                       "subscriptions": [], "capabilities": [],
                                       "lifecycle": {"kind": "passive", "order": 0}}}))
        sys.stdout.flush()
"#,
    )
    .unwrap();

    let err = match ForeignPlugin::connect("python", vec![script.to_string_lossy().into_owned()]) {
        Ok(_) => panic!("a plugin on protocol 7.7.7 must be refused"),
        Err(e) => e,
    };
    assert!(
        matches!(err, SdkError::UnsupportedProtocolVersion { .. }),
        "expected a protocol refusal, got {err:?}"
    );
    let _ = std::fs::remove_file(&script);
}

// ---- Criterion 3: a conformance suite any language can run against -------

/// Criterion 3. The suite is a single command and it passes for the Python
/// binding. This test drives the same code path the binary does.
#[test]
fn criterion_03_python_binding_passes_the_conformance_suite() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_atlas-conformance"))
        .args(["python", &pricing_plugin()])
        .output()
        .expect("conformance suite must run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "suite rejected the reference binding:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("CONFORMANT"), "stdout was: {stdout}");
    assert!(stdout.contains("pricing v1.0.0"), "stdout was: {stdout}");
}

/// Criterion 3, negative. The suite must be able to fail, or it proves nothing.
#[test]
fn criterion_03_suite_rejects_a_nonconformant_plugin() {
    // A structurally valid plugin that claims a contract but never answers it.
    // The capability is declared so the manifest itself is sound: the refusal
    // has to come from the contract going unanswered, not from a shape error.
    let script = dir().join("liar.py");
    std::fs::write(
        &script,
        r#"import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m["type"] == "hello":
        print(json.dumps({"type": "manifest", "protocol": "1.0.0",
                          "manifest": {"id": "liar", "version": "1.0.0",
                                       "contracts_provided": [{"id": "never.served", "version": "1.0.0"}],
                                       "contracts_required": [], "subscriptions": [],
                                       "capabilities": [{"name": "lie", "requires": "never.served"}],
                                       "lifecycle": {"kind": "passive", "order": 0}}}))
        sys.stdout.flush()
"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_atlas-conformance"))
        .args(["python", &script.to_string_lossy()])
        .output()
        .expect("suite must run");

    assert!(
        !output.status.success(),
        "the suite must reject a plugin that claims a contract and never serves it"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("NOT CONFORMANT"));
    let _ = std::fs::remove_file(&script);
}

// ---- Criterion 4: a second language runs inside the real runtime ---------

/// Criterion 4. The Python plugin registers, resolves a contract, and consumes
/// an event — through the ordinary runtime, alongside a native plugin.
#[test]
fn criterion_04_foreign_plugin_participates_in_the_real_runtime() {
    let mut runtime = Runtime::new();
    runtime.discover(vec![
        Box::new(atlas_sdk::plugins::inventory::Inventory::new()),
        foreign(),
    ]);

    runtime.validate().expect("both manifests validate");
    runtime.register().expect("the foreign plugin binds");
    runtime.initialize().expect("the foreign plugin starts");

    // It is a first-class member of the running system.
    let ids = runtime.plugin_ids();
    assert!(ids.contains(&"pricing".to_string()), "ids were {ids:?}");
    assert_eq!(runtime.contracts().len(), 2, "native + foreign contracts");

    // Contract resolution: the runtime resolves the foreign contract by
    // identity, exactly as it would a native one.
    let response = runtime
        .resolve_contract(
            &ops(),
            &ContractId::new("pricing.quote"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(3))]),
        )
        .expect("the foreign contract resolves");

    assert_eq!(response.get("total").and_then(Value::as_num), Some(58.5));
    assert_eq!(
        response.get("currency").and_then(Value::as_str),
        Some("EGP")
    );
    // Authority crossed the boundary intact.
    assert_eq!(
        response.get("principal").and_then(Value::as_str),
        Some("ops@atlas")
    );

    // Event consumption: an event the foreign plugin declared is routed to it.
    runtime
        .publish(
            &ops(),
            &Event::new(
                "order.placed",
                1,
                Value::map(vec![("order_id", Value::str("ORD-2001"))]),
            ),
        )
        .expect("the foreign plugin accepts the event it declared");

    let audit = runtime.audit_records().join("\n");
    assert!(
        audit.contains("foreign call pricing.quote"),
        "the runtime must record the foreign call: {audit}"
    );

    let order = runtime.shutdown().expect("shutdown runs");
    assert!(
        order.contains(&"pricing".to_string()),
        "order was {order:?}"
    );
}

/// Criterion 4, the crux. Authority is enforced on a foreign contract by the
/// same code path as a native one — there is no special case for foreignness.
#[test]
fn criterion_04_authority_is_enforced_on_a_foreign_contract() {
    let mut runtime = Runtime::new();
    // pricing requires inventory.reserve, so a provider must be present for
    // validation to pass. It is not the one being called.
    runtime.discover(vec![
        Box::new(atlas_sdk::plugins::inventory::Inventory::new()),
        foreign(),
    ]);
    runtime.validate().unwrap();
    runtime.register().unwrap();
    runtime.initialize().unwrap();

    // No authority: refused, and the plugin is never reached.
    let err = runtime
        .resolve_contract(
            &Authority::new("intern@atlas", vec!["pricing.audit".into()]),
            &ContractId::new("pricing.quote"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(1))]),
        )
        .expect_err("an unauthorized caller must be refused");

    assert_eq!(
        err,
        SdkError::Unauthorized {
            principal: "intern@atlas".into(),
            capability: "pricing.quote".into(),
        }
    );
    assert!(
        runtime.audit_records().is_empty(),
        "nothing should have run"
    );
}

/// Criterion 4. A foreign plugin declaring an unmet requirement fails
/// validation through the identical rule a native plugin would.
#[test]
fn criterion_04_foreign_manifest_is_validated_by_the_same_rules() {
    let mut runtime = Runtime::new();
    runtime.discover(vec![foreign()]);
    let err = runtime
        .validate()
        .expect_err("pricing requires inventory.reserve, which nobody provides");
    assert_eq!(
        err,
        SdkError::MissingContract {
            plugin: "pricing".into(),
            contract: "inventory.reserve".into(),
            version: "1.0.0".into(),
        }
    );
}

/// Criterion 4. Satisfied alongside a native provider, the foreign plugin's
/// requirement resolves and the whole system composes.
#[test]
fn criterion_04_foreign_and_native_plugins_compose() {
    let mut runtime = Runtime::new();
    runtime.discover(vec![
        Box::new(atlas_sdk::plugins::inventory::Inventory::new()),
        foreign(),
    ]);
    runtime
        .validate()
        .expect("foreign requirement satisfied by a native provider");
    runtime.register().unwrap();
    runtime.initialize().unwrap();

    // Both contracts resolve through one registry.
    let native = runtime
        .resolve_contract(
            &Authority::new("ops@atlas", vec!["inventory.reserve".into()]),
            &ContractId::new("inventory.reserve"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(1))]),
        )
        .expect("native contract");
    let foreign = runtime
        .resolve_contract(
            &ops(),
            &ContractId::new("pricing.quote"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(1))]),
        )
        .expect("foreign contract");

    assert_eq!(
        native.get("served_by").and_then(Value::as_str),
        Some("inventory")
    );
    assert_eq!(
        foreign.get("served_by").and_then(Value::as_str),
        Some("pricing")
    );
}

/// Criterion 4, lifecycle. Shutdown reaches the foreign process and reaps it.
#[test]
fn criterion_04_foreign_plugin_shuts_down_and_is_reaped() {
    let mut plugin = ForeignPlugin::connect("python", vec![pricing_plugin()]).expect("handshake");
    assert!(!plugin.stopped());
    plugin.shutdown().expect("shutdown");
    assert!(plugin.stopped(), "the process must be reaped, not leaked");
}

// ---- The SDK core did not change to accommodate the foreign plugin -------

/// The crux of invariant 9, stated as a check. If a foreign plugin needed a
/// special case in the registry, the boundary would be a Rust object model.
#[test]
fn sdk_core_has_no_foreign_plugin_special_case() {
    for path in ["src/runtime.rs", "src/context.rs", "src/manifest.rs"] {
        let source = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
            .unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        for forbidden in ["ForeignPlugin", "foreign", "bridge", "python", "subprocess"] {
            assert!(
                !source.to_lowercase().contains(&forbidden.to_lowercase()),
                "{path} mentions '{forbidden}' — the runtime must not know about foreignness"
            );
        }
    }
}

// ---- Helpers --------------------------------------------------------------

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Sanity: the context port is unchanged by any of this.
#[test]
fn context_port_is_unchanged() {
    let platform = Default::default();
    let ctx = Context::new(Authority::new("probe@atlas", vec![]), platform);
    assert_eq!(ctx.principal(), "probe@atlas");
}
