//! Architectural invariants, checked.
//!
//! The contract names twelve invariants that must remain true. Most of them are
//! already enforced elsewhere; this file checks the ones that are not, and it
//! checks them by behaviour wherever behaviour is reachable through the public
//! API — falling back to reading the source only where a property has no
//! observable consequence, and never trusting a comment.
//!
//! Each test names the invariant it defends. If someone changes the SDK in a
//! way that breaks one, a named test fails.
//!
//! Invariant 10 is not yet implemented (Connect does not exist), so it is
//! recorded as absent rather than claimed as satisfied. An invariant that has
//! never been violated has also never been tested.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use atlas_sdk::context::ContractFn;
use atlas_sdk::identity::{
    Authority, Capability, ContractDecl, ContractId, Event, EventDecl, Version,
};
use atlas_sdk::{Context, Manifest, Plugin, Runtime, SdkError, Value};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn source(relative: &str) -> String {
    std::fs::read_to_string(repo().join(relative))
        .unwrap_or_else(|e| panic!("cannot read {relative}: {e}"))
}

// ---- 1. The SDK owns orchestration ---------------------------------------

/// Invariant 1. The host may supply plugins and environment, but the lifecycle
/// methods live on the Runtime and nowhere else.
#[test]
fn invariant_01_sdk_owns_orchestration() {
    let host = source("src/main.rs");
    for forbidden in ["fn validate", "fn register", "fn initialize", "fn shutdown"] {
        assert!(
            !host.contains(forbidden),
            "src/main.rs defines '{forbidden}' — the host must not implement lifecycle"
        );
    }
    // And the runtime exposes them all.
    let runtime = source("src/runtime.rs");
    for step in [
        "pub fn discover",
        "pub fn validate",
        "pub fn register",
        "pub fn initialize",
        "pub fn expose_capabilities",
        "pub fn resolve_contract",
        "pub fn publish",
        "pub fn shutdown",
    ] {
        assert!(runtime.contains(step), "Runtime is missing {step}");
    }
}

// ---- 3. No direct plugin-to-plugin coupling -------------------------------

/// Invariant 3, restated from the plugin side: no plugin module may name
/// another plugin module in a `use`.
#[test]
fn invariant_03_no_plugin_imports_another_plugin() {
    for (plugin, forbidden) in [
        ("src/plugins/orders.rs", "inventory"),
        ("src/plugins/inventory.rs", "orders"),
    ] {
        let text = source(plugin);
        let leaks: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("use "))
            .filter(|l| l.contains(forbidden))
            .collect();
        assert!(leaks.is_empty(), "{plugin} imports {forbidden}: {leaks:?}");
    }
}

// ---- 4. Contracts and events are the only channels ------------------------

/// Invariant 4. A fact published with nobody listening succeeds and delivers to
/// none, because failing would make the publisher's success depend on the
/// subscriber set — a coupling the invariant forbids.
///
/// This was a real violation: `publish` returned `Err(NoSubscribers)`.
#[test]
fn invariant_04_publishing_does_not_depend_on_the_subscriber_set() {
    let runtime = Runtime::new();
    let publisher = Authority::new("ops@atlas", vec![]);

    // Nobody is listening. That is not an error.
    runtime
        .publish(&publisher, &Event::new("nobody.cares", 1, Value::Null))
        .expect("an event with no subscribers must still publish successfully");

    // And it is observable, so a silent drop is not invisible.
    let audit = runtime.audit_records().join("\n");
    assert!(audit.contains("nobody.cares"), "audit: {audit}");
    assert!(
        audit.contains("had no subscriber"),
        "an undelivered event must be recorded: {audit}"
    );
}

// ---- 5. Capabilities are explicit and machine-readable --------------------

/// Invariant 5. A plugin may not offer a contract without declaring what
/// authority invoking it requires. Otherwise the runtime would have to invent
/// the rule, and a caller holding a grant named after the contract would be let
/// in.
///
/// This was a real violation: `required_authority_for` fell back to the contract
/// id, so a capability-less contract was callable by anyone who guessed its
/// name.
#[test]
fn invariant_05_a_provided_contract_must_declare_its_authority() {
    let bad = Manifest::new("sneaky", Version::new(1, 0, 0))
        .provides(ContractDecl::new("sneaky.backdoor", Version::new(1, 0, 0)));

    let err = bad
        .validate()
        .expect_err("a contract with no declared capability must be refused");
    assert_eq!(
        err,
        "contract 'sneaky.backdoor' is provided but no capability declares the authority required to invoke it"
    );

    // Declaring it makes the plugin valid again.
    let good = bad
        .clone()
        .capability(Capability::new("sneaky:use", "sneaky.backdoor"));
    assert!(good.validate().is_ok());
}

/// Invariant 5, second half. The runtime must not carry a fallback that invents
/// an authority requirement — not under any spelling.
///
/// This is checked through the public API rather than by scanning the source,
/// because a text scan only catches the one fallback it was written to look
/// for. Here the handler is a real, registered contract, so if the runtime
/// invented a rule the call would succeed and the handler would run; if it
/// refuses, the handler provably never ran.
#[test]
fn invariant_05_runtime_does_not_invent_an_authority_rule() {
    struct Undeclared {
        manifest: Manifest,
        invoked: Rc<Cell<bool>>,
    }

    impl Plugin for Undeclared {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }

        fn contracts(&self) -> Vec<(ContractDecl, ContractFn)> {
            let invoked = Rc::clone(&self.invoked);
            let handler: ContractFn = Rc::new(move |_ctx: &Context, _p: Value| {
                invoked.set(true);
                Ok(Value::str("served"))
            });
            let decl = ContractDecl::new("undeclared.provide", Version::new(1, 0, 0));
            vec![(decl, handler)]
        }
    }

    let invoked = Rc::new(Cell::new(false));
    let mut runtime = Runtime::new();
    runtime.discover(vec![Box::new(Undeclared {
        // Exactly the manifest `validate` refuses: a contract with no declared
        // authority requirement.
        manifest: Manifest::new("undeclared", Version::new(1, 0, 0)).provides(ContractDecl::new(
            "undeclared.provide",
            Version::new(1, 0, 0),
        )),
        invoked: Rc::clone(&invoked),
    })]);

    // validate() is deliberately skipped: the question is what the runtime does
    // when it is not holding the guarantee, and it must refuse there too.
    runtime
        .register()
        .expect("the contract really is registered");
    runtime.initialize().unwrap();

    // A caller holding a grant named after the contract is still refused.
    let err = runtime
        .resolve_contract(
            &Authority::new("sneaky@atlas", vec!["undeclared.provide".into()]),
            &ContractId::new("undeclared.provide"),
            Version::new(1, 0, 0),
            Value::Null,
        )
        .expect_err("the runtime must not invent an authority requirement");

    assert_eq!(
        err,
        SdkError::ContractNotFound {
            contract: "undeclared.provide".into(),
            version: "1.0.0".into(),
        }
    );
    assert!(
        !invoked.get(),
        "the handler ran, so the contract was served under an invented rule"
    );
}

// ---- 6. Lifecycle is SDK-managed -------------------------------------------

/// Invariant 6. Calling lifecycle steps out of order is refused rather than
/// silently tolerated.
#[test]
fn invariant_06_lifecycle_is_sdk_managed() {
    let mut runtime = Runtime::new();
    runtime.shutdown().expect("shutdown with nothing running");

    let err = runtime
        .initialize()
        .expect_err("initializing a shut-down runtime must be refused");
    assert!(
        err.to_string().contains("already shut down"),
        "expected an illegal-state refusal, got {err}"
    );
}

// ---- 7. Runtime context flows through SDK-defined ports -------------------

/// Invariant 7. Platform wiring is `pub(crate)`, so a plugin cannot reach the
/// registry through the type system rather than through discipline.
#[test]
fn invariant_07_context_ports_are_the_only_door() {
    let context = source("src/context.rs");
    for fn_name in [
        "fn register_contract",
        "fn register_subscription",
        "fn set_config",
    ] {
        let line = context
            .lines()
            .find(|l| l.contains(fn_name))
            .unwrap_or_else(|| panic!("{fn_name} should still exist"));
        assert!(
            line.contains("pub(crate)"),
            "{fn_name} must be pub(crate), found: {line}"
        );
    }
    // Everything a plugin can reach is on Context.
    for port in [
        "pub fn authority",
        "pub fn principal",
        "pub fn call",
        "pub fn publish",
        "pub fn audit",
        "pub fn log",
        "pub fn config",
    ] {
        assert!(context.contains(port), "Context is missing {port}");
    }
}

// ---- 8. The host supplies a surface, not a runtime model ------------------

/// Invariant 8. The host may load components — that is its job — but must not
/// reimplement validation, registration or ordering.
#[test]
fn invariant_08_host_does_not_redefine_the_runtime_model() {
    let host = source("src/main.rs");
    // Loading a plugin is the host's legitimate job.
    assert!(
        host.contains("runtime.discover("),
        "the host should hand plugins to the runtime"
    );
    // But it must not second-guess validation outcomes.
    assert!(
        !host.contains("DuplicatePluginId") && !host.contains("MissingContract"),
        "the host is inspecting registry refusals — it should let the runtime decide"
    );
}

// ---- 9. The boundary is language-independent ------------------------------

/// Invariant 9. The SDK core must not know that a plugin can be foreign, and
/// the boundary must be documented outside the code.
#[test]
fn invariant_09_boundary_is_protocol_not_object_model() {
    for path in ["src/runtime.rs", "src/context.rs", "src/manifest.rs"] {
        let text = source(path).to_lowercase();
        for forbidden in ["foreign", "bridge", "python", "subprocess"] {
            assert!(
                !text.contains(forbidden),
                "{path} mentions '{forbidden}' — the core must not know about foreignness"
            );
        }
    }
    assert!(
        repo().join("protocol").join("PROTOCOL.md").exists(),
        "the boundary must be documented as a protocol"
    );
}

// ---- 10. Connect owns inter-host mechanics --------------------------------

/// Invariant 10 is NOT YET IMPLEMENTED. C-003 is still draft. This test records
/// the absence honestly rather than asserting a guarantee that does not exist,
/// and it will start failing loudly the day a Connect module appears so the
/// claim can then be checked for real.
#[test]
fn invariant_10_connect_layer_is_not_yet_implemented() {
    let has_connect = Path::new(&repo().join("src").join("connect.rs")).exists()
        || source("src/lib.rs").contains("pub mod connect");

    assert!(
        !has_connect,
        "a Connect layer now exists. Invariant 10 has moved from 'not implemented' to \
         'implemented' and must now be VERIFIED: that it carries authority semantics \
         (master/reader/proposer) and contains no domain vocabulary. Replace this test \
         with one that proves that."
    );
}

// ---- 11. The SDK stays smaller than what is built on it -------------------

/// Invariant 11. The platform must not be carrying domain vocabulary. The word
/// list is deliberately narrow: it names business domains the contract lists as
/// examples the SDK must NOT understand.
#[test]
fn invariant_11_sdk_carries_no_domain_vocabulary() {
    let banned = [
        "attendance",
        "payroll",
        "employee",
        "salary",
        "invoice",
        "timesheet",
        "workforce",
        "procurement",
        "tenant",
        "customer",
        "ledger",
    ];
    for path in [
        "src/runtime.rs",
        "src/context.rs",
        "src/manifest.rs",
        "src/protocol.rs",
        "src/bridge.rs",
    ] {
        let text = source(path).to_lowercase();
        for word in banned {
            assert!(
                !text.contains(word),
                "{path} contains domain vocabulary '{word}' — the SDK must not absorb domain concepts"
            );
        }
    }
}

// ---- 12. Room for unimagined applications ---------------------------------

/// Invariant 12, in the only way it can be checked: the SDK must be able to
/// accept a plugin that declares nothing the SDK already knows about. A new
/// contract, capability and event identity must compose without an SDK change.
#[test]
fn invariant_12_an_unimagined_plugin_needs_no_sdk_change() {
    struct Novel {
        manifest: Manifest,
    }
    impl Plugin for Novel {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }

        fn contracts(&self) -> Vec<(ContractDecl, ContractFn)> {
            let handler: ContractFn =
                Rc::new(|ctx: &Context, _p: Value| Ok(Value::str(ctx.principal())));
            let decl = ContractDecl::new("some.unheard_of.contract", Version::new(4, 2, 0));
            vec![(decl, handler)]
        }
    }

    let novel = Novel {
        manifest: Manifest::new("a.plugin.that.did.not.exist.yet", Version::new(7, 3, 1))
            .provides(ContractDecl::new(
                "some.unheard_of.contract",
                Version::new(4, 2, 0),
            ))
            .subscribes(EventDecl::new("a.brand.new.event", 99))
            .capability(Capability::new("novel:cap", "some.unheard_of.contract")),
    };

    let mut runtime = Runtime::new();
    runtime.discover(vec![Box::new(novel)]);
    runtime
        .validate()
        .expect("a plugin the SDK has never seen must still be valid");
    runtime.register().unwrap();
    runtime.initialize().unwrap();

    assert_eq!(
        runtime.plugin_ids(),
        vec!["a.plugin.that.did.not.exist.yet"]
    );
    assert_eq!(runtime.contracts(), vec!["some.unheard_of.contract v4.2.0"]);

    // A brand-new contract composes and runs, carrying the caller's authority.
    assert_eq!(
        runtime
            .resolve_contract(
                &Authority::new("someone@atlas", vec!["some.unheard_of.contract".into()]),
                &ContractId::new("some.unheard_of.contract"),
                Version::new(4, 2, 0),
                Value::Null,
            )
            .expect("a caller holding the declared authority is served"),
        Value::str("someone@atlas")
    );

    // And the authority it declared is enforced without any SDK change.
    assert_eq!(
        runtime
            .resolve_contract(
                &Authority::new("stranger@atlas", vec![]),
                &ContractId::new("some.unheard_of.contract"),
                Version::new(4, 2, 0),
                Value::Null,
            )
            .expect_err("a caller without the declared authority is refused"),
        SdkError::Unauthorized {
            principal: "stranger@atlas".into(),
            capability: "some.unheard_of.contract".into(),
        }
    );
}
