//! Acceptance tests for contract item C-004.
//!
//! A cluster is a declaration, so most of what is worth testing here is that it
//! stays one: it is validated or refused, and then it is inert. The sharp edge
//! is criterion 5 — if being grouped changed anything a plugin could observe,
//! clusters would be a second model wearing the plugin model's clothes.

use std::cell::Cell;
use std::rc::Rc;

use atlas_sdk::identity::{Capability, ContractDecl, ContractId, Event, Version};
use atlas_sdk::plugins::inventory::Inventory;
use atlas_sdk::plugins::orders::{ops_authority, Orders, ORDER_PLACED};
use atlas_sdk::{
    Cluster, ClusterRelation, Context, Manifest, Plugin, Runtime, SdkError, SdkResult, Value,
};

// ---- helpers --------------------------------------------------------------

/// A plugin that exists in order to be grouped. Nothing about it mentions a
/// cluster: one manifest, one declared contract, the ordinary lifecycle.
struct Member {
    manifest: Manifest,
    started: Rc<Cell<bool>>,
}

impl Member {
    fn new(id: &str, contract: &str, capability: &str, started: &Rc<Cell<bool>>) -> Self {
        Member {
            manifest: Manifest::new(id, Version::new(1, 0, 0))
                .provides(ContractDecl::new(contract, Version::new(1, 0, 0)))
                .capability(Capability::new(capability, contract)),
            started: Rc::clone(started),
        }
    }
}

impl Plugin for Member {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn on_initialize(&mut self, _ctx: &Context) -> SdkResult<()> {
        self.started.set(true);
        Ok(())
    }
}

fn group_of(started: &Rc<Cell<bool>>) -> (Box<dyn Plugin>, Box<dyn Plugin>) {
    (
        Box::new(Member::new("a.one", "svc.one", "shared:thing", started)),
        Box::new(Member::new("a.two", "svc.two", "shared:thing", started)),
    )
}

/// The contract `orders` requires and `inventory` provides, named as a string
/// by the test rather than imported from either plugin's module.
fn reserve_decl() -> ContractDecl {
    ContractDecl::new("inventory.reserve", Version::new(1, 0, 0))
}

// ---- Criterion 1: a cluster groups plugins, and the registry validates it --

/// Criterion 1. Two plugins grouped under one name, validated as a unit, and
/// observable afterwards.
#[test]
fn criterion_01_a_cluster_groups_plugins_and_the_registry_validates_it() {
    let started = Rc::new(Cell::new(false));
    let (one, two) = group_of(&started);

    let mut runtime = Runtime::new();
    runtime.discover(vec![one, two]);
    runtime.declare_cluster(
        Cluster::new("group.a", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.two"),
    );

    runtime
        .validate()
        .expect("a cluster of discovered plugins is valid");
    runtime.register().expect("grouping binds nothing extra");
    runtime
        .initialize()
        .expect("grouped plugins start like any others");
    assert!(started.get());

    // The group is observable from outside, which is the only capability it has.
    assert_eq!(runtime.clusters(), vec!["group.a"]);
    assert_eq!(runtime.cluster_of("a.one").as_deref(), Some("group.a"));
    assert_eq!(runtime.cluster_of("a.two").as_deref(), Some("group.a"));
    assert_eq!(runtime.cluster_of("ungrouped"), None);

    // Grouping did not merge them: they are still two separately-identified
    // plugins in the running system.
    assert_eq!(runtime.plugin_ids(), vec!["a.one", "a.two"]);
    assert_eq!(
        runtime.contracts(),
        vec!["svc.one v1.0.0", "svc.two v1.0.0"]
    );
}

// ---- Criterion 2: declared relationships are validated before anything runs -

/// Criterion 2, positive. Both relationship kinds are declared against the real
/// manifests and both hold: the two `Member` plugins really do declare
/// `shared:thing`, and `orders` really does get its `inventory.reserve`
/// requirement from a member of its own group.
#[test]
fn criterion_02_declared_relationships_are_checked_against_real_manifests() {
    let started = Rc::new(Cell::new(false));
    let (one, two) = group_of(&started);

    let mut runtime = Runtime::new();
    runtime.discover(vec![one, two]);
    runtime.declare_cluster(
        Cluster::new("group.a", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.two")
            .relation(ClusterRelation::shared_capability(
                "shared:thing",
                &["a.one", "a.two"],
            )),
    );
    runtime
        .validate()
        .expect("both members declare the shared capability");

    // And on the real plugins: orders requires inventory.reserve, and
    // inventory — also a member — provides it.
    let mut commerce = Runtime::new();
    commerce.discover(vec![Box::new(Inventory::new()), Box::new(Orders::new())]);
    commerce.declare_cluster(
        Cluster::new("commerce", Version::new(1, 0, 0))
            .member("inventory")
            .member("orders")
            .relation(ClusterRelation::depends_on("orders", reserve_decl())),
    );
    commerce
        .validate()
        .expect("the group is closed over that dependency");
    commerce
        .register()
        .expect("a declared relationship binds nothing at runtime");
    commerce
        .initialize()
        .expect("the group starts as one system");
}

/// Criterion 2, negative: the shared capability is claimed for a member that
/// does not declare it. Every plugin manifest is individually valid and every
/// contract requirement is satisfiable, so only the cluster declaration can be
/// the cause — which is the proof that relationships are validated in the same
/// pass as everything else, before anything operates.
#[test]
fn criterion_02_a_shared_capability_a_member_does_not_declare_is_refused() {
    let started = Rc::new(Cell::new(false));
    let mut runtime = Runtime::new();
    runtime.discover(vec![
        Box::new(Member::new("a.one", "svc.one", "shared:thing", &started)),
        // Same contract discipline, different capability name.
        Box::new(Member::new("a.two", "svc.two", "other:thing", &started)),
    ]);
    runtime.declare_cluster(
        Cluster::new("group.a", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.two")
            .relation(ClusterRelation::shared_capability(
                "shared:thing",
                &["a.one", "a.two"],
            )),
    );

    let err = runtime
        .validate()
        .expect_err("a capability neither shared nor declared must be refused");
    assert_eq!(
        err,
        SdkError::UnmetClusterRelation {
            cluster: "group.a".into(),
            detail: "shared capability 'shared:thing' is not declared by member 'a.two'".into(),
        }
    );
    assert_eq!(
        err.to_string(),
        "cluster 'group.a' relationship unmet: shared capability 'shared:thing' is not declared by member 'a.two'"
    );
    assert!(!started.get(), "validation refused, so nothing started");
}

/// Criterion 2, negative. `DependsOn` is not a restatement of "this plugin
/// requires that contract" — it asserts the provider is *inside* the group.
/// Here the requirement is real and satisfied, but by a plugin the cluster does
/// not contain, so the declaration is refused.
#[test]
fn criterion_02_a_dependency_served_from_outside_the_cluster_is_refused() {
    let mut runtime = Runtime::new();
    runtime.discover(atlas_sdk::default_system());
    runtime.declare_cluster(
        Cluster::new("front", Version::new(1, 0, 0))
            .member("orders")
            .relation(ClusterRelation::depends_on("orders", reserve_decl())),
    );

    // Nothing else is wrong with this system: orders' requirement is satisfied
    // by inventory, which is a perfectly good registered plugin.
    let err = runtime
        .validate()
        .expect_err("a dependency served from outside the group must be refused");
    assert_eq!(
        err,
        SdkError::UnmetClusterRelation {
            cluster: "front".into(),
            detail: "contract 'inventory.reserve' v1.0.0 is required by 'orders' but provided by 'inventory', outside this cluster".into(),
        }
    );
}

// ---- Criterion 3: cross-cluster contract and event exchange ----------------

/// Criterion 3. `orders` sits in one cluster, `inventory` in another. An event
/// delivered to `orders` makes it resolve `inventory.reserve` across the
/// boundary, and the audit trail proves both hops happened.
#[test]
fn criterion_03_a_contract_and_an_event_cross_a_cluster_boundary() {
    let mut runtime = Runtime::new();
    runtime.discover(atlas_sdk::default_system());
    runtime
        .declare_cluster(Cluster::new("front", Version::new(1, 0, 0)).member("orders"))
        .declare_cluster(Cluster::new("back", Version::new(1, 0, 0)).member("inventory"));

    runtime
        .validate()
        .expect("two one-member clusters are valid");
    runtime.register().unwrap();
    runtime.initialize().unwrap();

    assert_eq!(runtime.cluster_of("orders").as_deref(), Some("front"));
    assert_eq!(runtime.cluster_of("inventory").as_deref(), Some("back"));

    // The event's subscriber is in `front`; the contract it resolves is
    // provided by a plugin in `back`. No relationship was declared between the
    // two groups — clusters neither require one nor forbid the hop.
    runtime
        .publish(
            &ops_authority(),
            &Event::new(
                ORDER_PLACED,
                1,
                Value::map(vec![("order_id", Value::str("ORD-3000"))]),
            ),
        )
        .expect("the event reaches its subscriber in the other cluster");

    let audit = runtime.audit_records().join("\n");
    assert!(
        audit.contains("order ORD-3000 fulfilled"),
        "the subscriber in 'front' did not receive the event: {audit}"
    );
    assert!(
        audit.contains("call inventory.reserve v1.0.0 provider=inventory"),
        "no contract call crossed from 'front' to 'back': {audit}"
    );

    // And the provider served the caller under the caller's own authority,
    // across the boundary, unchanged.
    let response = runtime
        .resolve_contract(
            &ops_authority(),
            &ContractId::new("inventory.reserve"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(4))]),
        )
        .expect("a contract in one cluster resolves from outside it");
    assert_eq!(
        response.get("principal").and_then(Value::as_str),
        Some("ops@atlas")
    );
}

/// Criterion 3, the import half. A cluster boundary must not become a module
/// edge. The two groups sit over two plugin modules that cannot see each other:
/// the consumer names a contract identity, and nothing else.
#[test]
fn criterion_03_a_cluster_boundary_does_not_become_an_import() {
    let consumer = include_str!("../src/plugins/orders.rs");
    let provider = include_str!("../src/plugins/inventory.rs");

    let imports_other_module = |src: &str, other: &str| -> Vec<String> {
        src.lines()
            .map(str::trim)
            .filter(|line| line.starts_with("use "))
            .filter(|line| line.contains(other))
            .map(str::to_string)
            .collect()
    };

    assert!(
        imports_other_module(consumer, "inventory").is_empty(),
        "the consumer's cluster must not import the provider's module"
    );
    assert!(
        imports_other_module(provider, "orders").is_empty(),
        "the provider's cluster must not import the consumer's module"
    );
}

// ---- Criterion 4: an invalid cluster is refused and does not start ---------

/// Criterion 4. Structurally invalid, refused with a stable rendered message,
/// and refused before anything started: the member would abort if any lifecycle
/// step ran, and this test stops at the refusal.
#[test]
fn criterion_04_an_invalid_cluster_is_refused_deterministically_and_does_not_start() {
    struct MustNotStart(Manifest);
    impl Plugin for MustNotStart {
        fn manifest(&self) -> &Manifest {
            &self.0
        }
        fn on_initialize(&mut self, _ctx: &Context) -> SdkResult<()> {
            panic!("a member of a refused cluster must never be started");
        }
    }

    let mut runtime = Runtime::new();
    runtime.discover(vec![Box::new(MustNotStart(
        Manifest::new("bad.member", Version::new(1, 0, 0))
            .provides(ContractDecl::new("bad.provide", Version::new(1, 0, 0)))
            .capability(Capability::new("bad:provide", "bad.provide")),
    ))]);
    // A cluster with no members groups nothing, so it is not a declaration of
    // anything.
    runtime.declare_cluster(Cluster::new("bad.group", Version::new(1, 0, 0)));

    let err = runtime
        .validate()
        .expect_err("a cluster declaring no members must be refused");
    assert_eq!(
        err,
        SdkError::InvalidCluster {
            cluster: "bad.group".into(),
            reason: "cluster declares no members".into(),
        }
    );
    assert_eq!(
        err.to_string(),
        "invalid cluster 'bad.group': cluster declares no members"
    );

    // Deterministic: the same declaration refused again renders identically.
    assert_eq!(runtime.validate().unwrap_err().to_string(), err.to_string());

    // The refusal was the cluster's, not the system's. Nothing beyond `validate`
    // ran, so the plugin's tripwire was never reached.
    assert_eq!(runtime.plugin_ids(), vec!["bad.member"]);
    assert!(runtime.audit_records().is_empty());
}

/// Criterion 4. A cluster may only name plugins the host actually discovered.
#[test]
fn criterion_04_a_cluster_naming_an_undiscovered_plugin_is_refused() {
    let mut runtime = Runtime::new();
    runtime
        .declare_cluster(Cluster::new("group", Version::new(1, 0, 0)).member("never.discovered"));

    let err = runtime
        .validate()
        .expect_err("a member that was never discovered cannot be grouped");
    assert_eq!(
        err,
        SdkError::UnknownClusterMember {
            cluster: "group".into(),
            member: "never.discovered".into(),
        }
    );
    assert_eq!(
        err.to_string(),
        "cluster 'group' names member 'never.discovered', which was never discovered"
    );
}

/// Criterion 4. Two clusters may not share one identity, and that is checked
/// before the second one is judged on its own merits.
#[test]
fn criterion_04_duplicate_cluster_identity_is_refused() {
    let started = Rc::new(Cell::new(false));
    let (one, two) = group_of(&started);

    let mut runtime = Runtime::new();
    runtime.discover(vec![one, two]);
    runtime.declare_cluster(
        Cluster::new("twins", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.two"),
    );
    runtime.declare_cluster(
        Cluster::new("twins", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.two"),
    );

    let err = runtime
        .validate()
        .expect_err("two clusters may not share one identity");
    assert_eq!(err, SdkError::DuplicateClusterId { id: "twins".into() });
    assert_eq!(
        err.to_string(),
        "duplicate cluster identity 'twins': already declared, rejected the second"
    );
    assert!(!started.get());
}

// ---- Criterion 5: grouping changes no plugin behaviour ---------------------

/// Everything a running system exposes, as one comparable record. Used to prove
/// that grouping changes none of it.
#[derive(Debug, PartialEq)]
struct Observed {
    plugin_ids: Vec<String>,
    contracts: Vec<String>,
    capabilities: Vec<String>,
    contract_response: Value,
    audit: Vec<String>,
    logs: Vec<String>,
    shutdown_order: Vec<String>,
}

fn observe(runtime: &mut Runtime) -> Observed {
    runtime.validate().expect("the system is valid");
    runtime.register().expect("bindings bind");
    runtime.initialize().expect("plugins start");

    let contract_response = runtime
        .resolve_contract(
            &ops_authority(),
            &ContractId::new("inventory.reserve"),
            Version::new(1, 0, 0),
            Value::map(vec![("quantity", Value::num(2))]),
        )
        .expect("an authorized caller resolves");

    runtime
        .publish(
            &ops_authority(),
            &Event::new(
                ORDER_PLACED,
                1,
                Value::map(vec![("order_id", Value::str("ORD-77"))]),
            ),
        )
        .expect("the event reaches its subscriber");

    let capabilities = runtime
        .expose_capabilities()
        .iter()
        .map(|c| format!("{}|{}|{}", c.plugin, c.name, c.requires))
        .collect();

    Observed {
        plugin_ids: runtime.plugin_ids(),
        contracts: runtime.contracts(),
        capabilities,
        contract_response,
        audit: runtime.audit_records(),
        logs: runtime.log_records(),
        shutdown_order: runtime.shutdown().expect("shutdown runs"),
    }
}

/// Criterion 5, the sharp edge. The same two plugins, once ungrouped and once
/// grouped into two clusters. Every observable the runtime offers must be
/// identical, because only `validate` reads a cluster and it reads no plugin
/// state.
#[test]
fn criterion_05_grouping_changes_nothing_a_plugin_can_observe() {
    let mut bare = Runtime::new();
    bare.discover(atlas_sdk::default_system());
    let ungrouped = observe(&mut bare);

    let mut grouped = Runtime::new();
    grouped.discover(atlas_sdk::default_system());
    grouped
        .declare_cluster(Cluster::new("front", Version::new(1, 0, 0)).member("orders"))
        .declare_cluster(Cluster::new("back", Version::new(1, 0, 0)).member("inventory"));
    let clustered = observe(&mut grouped);

    // Not vacuous: the record holds real values, so this compares behaviour
    // rather than two empty structs.
    assert_eq!(ungrouped.plugin_ids, vec!["inventory", "orders"]);
    assert_eq!(ungrouped.contracts, vec!["inventory.reserve v1.0.0"]);
    assert_eq!(ungrouped.shutdown_order, vec!["orders", "inventory"]);
    assert_eq!(ungrouped.capabilities.len(), 2);
    assert!(!ungrouped.logs.is_empty());
    assert!(ungrouped
        .audit
        .iter()
        .any(|line| line.contains("reserved 2 for ops@atlas")));

    assert_eq!(ungrouped, clustered);

    // The grouping was real, so the comparison above was not the trivially
    // equal case of two runtimes that declared nothing.
    assert_eq!(grouped.clusters(), vec!["front", "back"]);
}
