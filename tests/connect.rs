//! Acceptance tests for contract item C-003, the Connect layer.
//!
//! Two hosts, two runtimes, one link. The thing worth proving is not that
//! something crossed — it is *who decided* that it could. So most of what is
//! asserted here about a refusal comes from the receiving host's own state: the
//! handler that would have run, the audit trail that would have recorded it,
//! and the capability the receiving host itself declared.

use std::cell::RefCell;
use std::rc::Rc;

use atlas_sdk::connect::{Link, Peer, Role, LINK_VERSION};
use atlas_sdk::identity::{Authority, ContractDecl, ContractId, Event, Version};
use atlas_sdk::plugins::relay::{compute_decl, signal_decl, Relay, RELAY_SIGNAL};
use atlas_sdk::{Runtime, SdkError, Value};

const HOST_A: &str = "host-a@atlas";
const HOST_B: &str = "host-b@atlas";

/// Host A: the initiating host. It runs the ordinary two-plugin system, so what
/// it advertises to a peer is a real surface and not an empty claim.
fn host_a() -> Runtime {
    let mut runtime = Runtime::new();
    runtime.discover(atlas_sdk::default_system());
    runtime.validate().expect("host A is a valid host");
    runtime.register().expect("bindings bind");
    runtime.initialize().expect("plugins start");
    runtime
}

/// Host B: the receiving host. Its own handle on what crossed, so a refusal can
/// be checked against this side rather than inferred from the sender.
fn host_b() -> (Runtime, Rc<RefCell<Vec<String>>>) {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let mut runtime = Runtime::new();
    runtime.discover(vec![Box::new(Relay::new(&observed))]);
    runtime.validate().expect("host B is a valid host");
    runtime.register().expect("bindings bind");
    runtime.initialize().expect("plugins start");
    (runtime, observed)
}

fn advert(a: &Runtime, b: &Runtime) -> (Peer, Peer) {
    (
        Peer::advertise(HOST_A, Version::new(1, 0, 0), a),
        Peer::advertise(HOST_B, Version::new(1, 0, 0), b),
    )
}

/// A link from A to B carrying exactly the authority B's own manifest declared
/// for `relay.compute`.
fn proposer_link(a: &Runtime, b: &Runtime) -> Link {
    let (mine, theirs) = advert(a, b);
    Link::open(
        mine,
        theirs,
        Role::Proposer,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect("two hosts speaking the same link version establish a link")
}

// ---- Criterion 1: two hosts, discovery, handshake, identity, versions -------

/// Criterion 1. Two independent runtimes, each advertising what its own
/// registry actually holds, negotiate a link. The advertisements are read out
/// of the registries rather than typed in by the test, so discovery is
/// describing two systems and not repeating the test's own expectations.
#[test]
fn criterion_01_two_hosts_establish_a_link_by_discovery_and_handshake() {
    let a = host_a();
    let (b, _) = host_b();
    let link = proposer_link(&a, &b);

    // Identity: each end knows which host it is and which host it addresses.
    assert_eq!(link.local().id, HOST_A);
    assert_eq!(link.remote().id, HOST_B);
    assert_eq!(link.local().version, Version::new(1, 0, 0));
    assert_eq!(link.remote().version, Version::new(1, 0, 0));

    // Discovery described host A's real surface: the contract `inventory`
    // provides. `orders` binds its subscription through `Plugin::subscriptions`
    // rather than declaring it in its manifest, so the declared event surface of
    // this host is empty — and discovery reports what the registry holds rather
    // than what a host might have hoped it held.
    assert_eq!(
        link.local().contracts,
        vec![ContractDecl::new(
            "inventory.reserve",
            Version::new(1, 0, 0)
        )]
    );
    assert!(link.local().events.is_empty());

    // And host B's, from the plugin on the far side of the link.
    assert_eq!(link.remote().contracts, vec![compute_decl()]);
    assert_eq!(link.remote().events, vec![signal_decl()]);

    // Version negotiation happened: both ends speak the same link version, and
    // the link reports the version they agreed on rather than assuming one.
    assert_eq!(link.local().link, LINK_VERSION);
    assert_eq!(link.remote().link, link.local().link);
    assert_eq!(link.link_version(), LINK_VERSION);

    // Not vacuous: both hosts really are running, separately, with their own
    // plugins and their own audit trails.
    assert_eq!(a.plugin_ids(), vec!["inventory", "orders"]);
    assert_eq!(b.plugin_ids(), vec!["relay"]);
    assert!(a.audit_records().is_empty());
    assert!(b.audit_records().is_empty());
}

/// Criterion 1, the negotiation refusal. A host speaking another link version
/// cannot be linked, and the refusal says what this host does speak. Half a
/// handshake is not a handshake.
#[test]
fn criterion_01_a_host_on_another_link_version_is_refused() {
    let a = host_a();
    let (b, _) = host_b();
    let (mine, mut theirs) = advert(&a, &b);
    theirs.link = Version::new(2, 0, 0);

    let err = Link::open(
        mine,
        theirs,
        Role::Proposer,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect_err("two ends that do not speak the same link version cannot link");

    assert_eq!(
        err,
        SdkError::UnsupportedLinkVersion {
            declared: "2.0.0".into(),
            supported: "1.0.0".into(),
        }
    );
    assert_eq!(
        err.to_string(),
        "connect link speaks version 2.0.0 which this host does not support (supported: 1.0.0)"
    );
}

/// Criterion 1, the identity half of the handshake. A link may not present
/// authority under a name the local host does not hold, or a host could speak
/// for another one, and a host may not be its own peer.
#[test]
fn criterion_01_a_link_may_not_speak_for_another_host() {
    let a = host_a();
    let (b, _) = host_b();
    let (mine, theirs) = advert(&a, &b);

    let err = Link::open(
        mine.clone(),
        theirs.clone(),
        Role::Proposer,
        Authority::new("host-z@atlas", vec!["relay.compute".into()]),
    )
    .expect_err("a link may not present someone else's identity");
    assert_eq!(
        err,
        SdkError::InvalidLink {
            host: HOST_A.into(),
            reason: format!(
                "the link presents authority 'host-z@atlas' but the local host is '{HOST_A}'"
            ),
        }
    );

    // A host may not be handed itself as its own peer.
    let err = Link::open(
        mine.clone(),
        mine,
        Role::Proposer,
        Authority::new(HOST_A, vec![]),
    )
    .expect_err("a host cannot be handed itself as its own peer");
    assert_eq!(
        err,
        SdkError::InvalidLink {
            host: HOST_A.into(),
            reason: format!("host '{HOST_A}' cannot be its own peer"),
        }
    );
}

// ---- Criterion 2: an explicit authority relationship -----------------------

/// Criterion 2. All three standings open a link and are carried on it, with the
/// grants the host chose to hand over. A reader is the one standing this layer
/// can act on: it holds nothing, so the receiving host refuses it, and a reader
/// cannot even be built holding something.
#[test]
fn criterion_02_the_link_carries_an_explicit_authority_relationship() {
    let a = host_a();
    let (b, observed) = host_b();
    let (mine, theirs) = advert(&a, &b);

    for role in [Role::Master, Role::Proposer, Role::Reader] {
        let link = Link::open(
            mine.clone(),
            theirs.clone(),
            role,
            Authority::new(HOST_A, vec![]),
        )
        .unwrap_or_else(|e| panic!("role {role:?} must be expressible on a link: {e}"));
        assert_eq!(link.role(), role, "the standing is carried on the link");
        assert_eq!(link.grants().principal, HOST_A);
    }

    // Master and Proposer are distinct declarations and neither is downgraded:
    // each is served exactly as the grant it carries allows.
    for role in [Role::Master, Role::Proposer] {
        let link = Link::open(
            mine.clone(),
            theirs.clone(),
            role,
            Authority::new(HOST_A, vec!["relay.compute".into()]),
        )
        .expect("a granting standing may carry grants");
        let reply = link
            .invoke(&b, &compute_decl().id, Version::new(1, 0, 0), Value::Null)
            .unwrap_or_else(|e| panic!("{role:?} holding the grant is served: {e}"));
        assert_eq!(reply.get("principal").and_then(Value::as_str), Some(HOST_A));
    }

    // A reader holds nothing, so the receiving host refuses it — and the
    // refusal is the receiver's, made against its own declared capability.
    let before = observed.borrow().len();
    let reader = Link::open(
        mine.clone(),
        theirs.clone(),
        Role::Reader,
        Authority::new(HOST_A, vec![]),
    )
    .expect("a reader is a legal standing");
    let err = reader
        .invoke(&b, &compute_decl().id, Version::new(1, 0, 0), Value::Null)
        .expect_err("a reader holds no grant, so it is refused");
    assert_eq!(
        err,
        SdkError::Unauthorized {
            principal: HOST_A.into(),
            capability: "relay.compute".into(),
        }
    );
    assert_eq!(
        observed.borrow().len(),
        before,
        "a refused reader must not have reached the handler: {:?}",
        observed.borrow()
    );

    // And a reader cannot be handed a grant in the first place: the standing
    // and the grant would contradict each other.
    let err = Link::open(
        mine,
        theirs,
        Role::Reader,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect_err("a reader cannot be granted anything");
    assert_eq!(
        err,
        SdkError::InvalidLink {
            host: HOST_A.into(),
            reason: "role 'reader' holds no grants, so it cannot be granted one".into(),
        }
    );
}

// ---- Criterion 3: the receiving host enforces authorization ----------------

/// Criterion 3, the sharp edge. The same contract on the same link, granted and
/// not granted. The grant is refused by the receiving host, and three
/// independent pieces of the *receiver's* state say so: its handler never ran,
/// its audit trail never recorded a call, and the capability named in the
/// refusal is the one its own manifest declared. The sender's own trail stays
/// empty, because nothing here authorised the call — the far end did.
#[test]
fn criterion_03_authorization_is_enforced_at_the_receiving_host() {
    let a = host_a();
    let (b, observed) = host_b();
    let (mine, theirs) = advert(&a, &b);

    // Granted. Served, and served under the authority the link presented.
    let granted = Link::open(
        mine.clone(),
        theirs.clone(),
        Role::Proposer,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect("a link carrying the receiver's own declared authority");
    let reply = granted
        .invoke(
            &b,
            &compute_decl().id,
            Version::new(1, 0, 0),
            Value::map(vec![("question", Value::num(1))]),
        )
        .expect("the receiving host serves a caller holding the grant it declared");
    assert_eq!(reply.get("principal").and_then(Value::as_str), Some(HOST_A));

    // Same link shape, same contract, same receiving host — but the sender holds
    // nothing the receiver declared.
    let ungranted = Link::open(
        mine,
        theirs,
        Role::Proposer,
        Authority::new(HOST_A, vec!["relay.compute.not.mine".into()]),
    )
    .expect("a link may carry a grant the receiver will refuse; that is the point");
    let err = ungranted
        .invoke(&b, &compute_decl().id, Version::new(1, 0, 0), Value::Null)
        .expect_err("the receiving host must refuse a grant it never declared");

    // The refusal names the receiver's own capability, so it cannot have been
    // produced by the sender checking its own intentions.
    assert_eq!(
        err,
        SdkError::Unauthorized {
            principal: HOST_A.into(),
            capability: "relay.compute".into(),
        }
    );
    assert_eq!(
        err.to_string(),
        "principal 'host-a@atlas' is not authorized for capability 'relay.compute'"
    );

    // The receiver's handler ran once — for the granted call — and not again.
    assert_eq!(
        observed.borrow().as_slice(),
        ["served relay.compute v1.0.0 principal=host-a@atlas"]
    );
    // The receiver's audit trail has exactly one call record, the granted one.
    assert_eq!(
        b.audit_records()
            .iter()
            .filter(|line| line.contains("relay.compute"))
            .count(),
        1,
        "the denied call must not appear in the receiver's audit: {:?}",
        b.audit_records()
    );
    // And the sender's own runtime recorded nothing, because the decision was
    // not the sender's to record.
    assert!(
        a.audit_records().is_empty(),
        "the sender must not log a decision the receiving host made: {:?}",
        a.audit_records()
    );
}

// ---- Criterion 4: identity and version preserved, mismatch refused ----------

/// Criterion 4. A contract and a fact cross the link with the identity and
/// version they were addressed with, and the far end serves them as itself:
/// the response names the far host's own contract, and the subscriber heard
/// the event identity and version it declared, with the payload intact.
#[test]
fn criterion_04_contracts_and_events_cross_with_identity_and_version_preserved() {
    let a = host_a();
    let (b, observed) = host_b();
    let link = proposer_link(&a, &b);

    let payload = Value::map(vec![
        ("question", Value::num(41)),
        ("echo", Value::List(vec![Value::str("a"), Value::str("b")])),
    ]);
    let reply = link
        .invoke(
            &b,
            &compute_decl().id,
            Version::new(1, 0, 0),
            payload.clone(),
        )
        .expect("a contract at the version the peer advertised is served");
    assert_eq!(
        reply.get("echo"),
        Some(&payload),
        "the payload must arrive exactly as it was sent"
    );

    // Named as a string by the test, not imported from the plugin module, so
    // the two ends agree by identity rather than by sharing a symbol.
    let event = Event::new(
        RELAY_SIGNAL,
        1,
        Value::map(vec![("note", Value::str("hello"))]),
    );
    link.publish(&b, &event)
        .expect("a fact at the version the peer subscribed to crosses the link");

    assert_eq!(
        observed.borrow().as_slice(),
        [
            "served relay.compute v1.0.0 principal=host-a@atlas",
            "heard relay.signal v1 principal=host-a@atlas",
        ]
    );
    assert!(
        b.audit_records()
            .iter()
            .any(|line| line.contains("relay.signal v1 handled")),
        "the far host's subscriber saw the event: {:?}",
        b.audit_records()
    );
}

/// Criterion 4, the refusal half. A version the peer never advertised is
/// refused, never silently served at the version that happens to be on offer.
/// Same discipline as a required contract: exact match or nothing.
#[test]
fn criterion_04_a_version_mismatch_is_refused_rather_than_coerced() {
    let a = host_a();
    let (b, observed) = host_b();
    let link = proposer_link(&a, &b);

    let err = link
        .invoke(&b, &compute_decl().id, Version::new(2, 0, 0), Value::Null)
        .expect_err("a version the peer never offered must be refused");
    assert_eq!(
        err,
        SdkError::VersionNotOffered {
            peer: HOST_B.into(),
            subject: "relay.compute".into(),
            requested: "2.0.0".into(),
            available: vec!["1.0.0".into()],
        }
    );
    assert_eq!(
        err.to_string(),
        "host 'host-b@atlas' does not offer 'relay.compute' v2.0.0 (offered: [1.0.0])"
    );

    // The event path negotiates the same way: a version the peer never
    // subscribed to is refused rather than dropped or coerced.
    let err = link
        .publish(&b, &Event::new(RELAY_SIGNAL, 2, Value::Null))
        .expect_err("an event version the peer never subscribed to is refused");
    assert_eq!(
        err,
        SdkError::VersionNotOffered {
            peer: HOST_B.into(),
            subject: "relay.signal".into(),
            requested: "2".into(),
            available: vec!["1".into()],
        }
    );

    // An identity the peer never advertised at all is refused the same way, and
    // nothing reached the far host.
    let err = link
        .invoke(
            &b,
            &ContractId::new("relay.unheard_of"),
            Version::new(1, 0, 0),
            Value::Null,
        )
        .expect_err("a contract the peer never advertised is refused");
    assert_eq!(
        err,
        SdkError::VersionNotOffered {
            peer: HOST_B.into(),
            subject: "relay.unheard_of".into(),
            requested: "1.0.0".into(),
            available: vec![],
        }
    );
    assert!(
        observed.borrow().is_empty(),
        "nothing crossed the link: {:?}",
        observed.borrow()
    );
    assert!(b.audit_records().is_empty());
}

// ---- Criterion 5: mechanics, and no meaning --------------------------------

/// Criterion 5, the behavioural half. A payload that speaks the link's own
/// language — naming a contract, a version, a principal and a grant of its own
/// — changes nothing. Identity and version came from the request, authority
/// came from the link, and the far host's handler reported the authority it was
/// actually called under while echoing the payload back untouched. Connect
/// routes; it does not read.
#[test]
fn criterion_05_connect_routes_a_payload_without_interpreting_it() {
    let a = host_a();
    let (b, _) = host_b();
    let (mine, theirs) = advert(&a, &b);

    // Every field a layer might be tempted to read out of a payload instead of
    // taking from the request.
    let impersonating = Value::map(vec![
        ("contract", Value::str("relay.compute")),
        ("version", Value::num(99)),
        ("principal", Value::str("root@atlas")),
        ("granted", Value::List(vec![Value::str("relay.compute")])),
    ]);

    let granted = Link::open(
        mine.clone(),
        theirs.clone(),
        Role::Master,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect("a link carrying the receiver's grant");
    let reply = granted
        .invoke(
            &b,
            &compute_decl().id,
            Version::new(1, 0, 0),
            impersonating.clone(),
        )
        .expect("the contract is still served");

    // The principal the far host saw is the link's, not the payload's.
    assert_eq!(reply.get("principal").and_then(Value::as_str), Some(HOST_A));
    // And the payload arrived whole, every field intact, including the ones
    // that tried to say something different.
    assert_eq!(reply.get("echo"), Some(&impersonating));

    // The same payload on a link the receiver never granted changes nothing
    // either: it is not a grant, and it does not become one by being asserted.
    let ungranted = Link::open(mine, theirs, Role::Master, Authority::new(HOST_A, vec![]))
        .expect("a link carrying no grant");
    let err = ungranted
        .invoke(&b, &compute_decl().id, Version::new(1, 0, 0), impersonating)
        .expect_err("a payload asserting a grant is still a payload");
    assert_eq!(
        err,
        SdkError::Unauthorized {
            principal: HOST_A.into(),
            capability: "relay.compute".into(),
        }
    );
}

// ---- The evidence clause, as a recorded run -------------------------------

/// The acceptance evidence in one pass: two hosts, a handshake, an authorized
/// contract and an authorized fact, then an unauthorized exchange refused by
/// the receiving host. Printed so the run itself is the record.
#[test]
fn evidence_two_host_handshake_authorized_exchange_and_refusal() {
    let a = host_a();
    let (b, observed) = host_b();
    let (mine, theirs) = advert(&a, &b);

    let link = Link::open(
        mine.clone(),
        theirs.clone(),
        Role::Master,
        Authority::new(HOST_A, vec!["relay.compute".into()]),
    )
    .expect("handshake");

    println!(
        "link {} --master--> {} (link v{})",
        link.local().id,
        link.remote().id,
        link.link_version()
    );
    println!("  host A advertises {:?}", link.local().contracts);
    println!("  host B advertises {:?}", link.remote().contracts);

    let reply = link
        .invoke(
            &b,
            &compute_decl().id,
            Version::new(1, 0, 0),
            Value::map(vec![("question", Value::num(2))]),
        )
        .expect("authorized exchange");
    println!("  authorized: {} v1.0.0 -> {}", compute_decl().id, reply);

    link.publish(&b, &Event::new(RELAY_SIGNAL, 1, Value::Null))
        .expect("authorized event");
    println!("  authorized: {} v1 delivered", RELAY_SIGNAL);

    let ungranted =
        Link::open(mine, theirs, Role::Master, Authority::new(HOST_A, vec![])).expect("handshake");
    let refused = ungranted
        .invoke(&b, &compute_decl().id, Version::new(1, 0, 0), Value::Null)
        .expect_err("unauthorized exchange");
    println!("  refused by host B: {refused}");
    println!("  host B saw: {:?}", observed.borrow());
}
