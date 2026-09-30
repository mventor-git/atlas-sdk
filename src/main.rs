//! `atlas-host` — the demo host.
//!
//! The host does four things: supply environment, hand plugins to the SDK,
//! trigger SDK-driven steps, and print what happened. It contains no lifecycle
//! logic of its own, which is the point.

use atlas_sdk::identity::{ContractId, Event, Version};
use atlas_sdk::plugin::Plugin;
use atlas_sdk::plugins::orders::{intern_authority, ops_authority, ORDER_PLACED};
use atlas_sdk::{Runtime, Value};

fn step(n: u8, name: &str, detail: String) {
    println!("\n[{n}] {name}\n    {detail}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Atlas-SDK demo host");

    // The host supplies environment. Not orchestration.
    let mut runtime = Runtime::with_config(&[("currency", "EGP")]);

    // ---- 1. discover -----------------------------------------------------
    let plugins: Vec<Box<dyn Plugin>> = atlas_sdk::default_system();
    let ids: Vec<String> = plugins.iter().map(|p| p.manifest().id.clone()).collect();
    runtime.discover(plugins);
    step(
        1,
        "discover",
        format!("host offered {} plugins: {:?}", ids.len(), ids),
    );

    // ---- 2. validate -----------------------------------------------------
    runtime.validate()?;
    step(
        2,
        "validate",
        format!("{} manifests accepted, dependencies resolvable", ids.len()),
    );

    // ---- 3. register -----------------------------------------------------
    runtime.register()?;
    step(
        3,
        "register",
        format!("bound contracts: {:?}", runtime.contracts()),
    );

    // ---- 4. initialize ---------------------------------------------------
    runtime.initialize()?;
    step(
        4,
        "initialize",
        "plugins started via SDK context ports".into(),
    );

    // ---- 5. expose capability -------------------------------------------
    let caps = runtime.expose_capabilities();
    let rendered: Vec<String> = caps
        .iter()
        .map(|c| format!("{} (requires {})", c.name, c.requires))
        .collect();
    step(
        5,
        "expose capability",
        format!("{} capabilities: {:?}", caps.len(), rendered),
    );

    // ---- 6. resolve contract --------------------------------------------
    let ops = ops_authority();
    let response = runtime.resolve_contract(
        &ops,
        &ContractId::new("inventory.reserve"),
        Version::new(1, 0, 0),
        Value::map(vec![("quantity", Value::num(3))]),
    )?;
    step(
        6,
        "resolve contract",
        format!(
            "ops@atlas -> inventory.reserve v1.0.0 -> {} (served under principal {})",
            response,
            response
                .get("principal")
                .and_then(Value::as_str)
                .unwrap_or("?")
        ),
    );

    // Authorization is enforced against what the provider declared.
    let denied = runtime.resolve_contract(
        &intern_authority(),
        &ContractId::new("inventory.reserve"),
        Version::new(1, 0, 0),
        Value::map(vec![("quantity", Value::num(1))]),
    );
    step(
        6,
        "  └ authority check",
        format!("intern@atlas refused: {}", denied.unwrap_err()),
    );

    // ---- 7. publish / consume event -------------------------------------
    runtime.publish(
        &ops,
        &Event::new(
            ORDER_PLACED,
            1,
            Value::map(vec![("order_id", Value::str("ORD-1001"))]),
        ),
    )?;
    step(
        7,
        "publish/consume event",
        "order.placed v1 delivered by the SDK; orders resolved the contract it requires"
            .to_string(),
    );

    // ---- 8. execute with context ----------------------------------------
    let audit = runtime.audit_records();
    step(
        8,
        "execute with context",
        format!(
            "{} audit records recorded under caller authority",
            audit.len()
        ),
    );

    // ---- 9. shutdown -----------------------------------------------------
    let order = runtime.shutdown()?;
    let repeat = runtime.shutdown()?;
    step(
        9,
        "shutdown",
        format!(
            "order {:?}; repeat call returned {} entries (idempotent)",
            order,
            repeat.len()
        ),
    );

    println!("\n--- audit trail ---");
    for line in &audit {
        println!("  {line}");
    }

    println!("\n--- plugin logs ---");
    for line in runtime.log_records() {
        println!("  {line}");
    }

    Ok(())
}
