//! The Connect layer: how two Atlas hosts find each other, agree to talk, and
//! say who may say what.
//!
//! Connect owns mechanics and the authority relationship between two hosts.
//! It owns no meaning. It does not read a payload, does not know what a
//! contract is for, and does not decide whether a call is allowed — the
//! receiving host does, from the capabilities its own plugins declared.
//!
//! There is no transport here. Two hosts link by being handed each other's
//! runtime, which keeps the proof about the authority model rather than about
//! inter-process plumbing. Nothing above this module knows how a link is
//! carried, so carrying one elsewhere is a change confined to this file.
//!
//! ponytail: one in-process link, synchronous, no transport, no persistence, no
//! redelivery, no reordering, no shared-state reconciliation. Ceiling: both
//! hosts must be in the same process and every operation returns before it
//! completes. Upgrade path: implement the same `Peer`/`Link` surface over a real
//! transport; the enforcement below is unchanged by that, because it never
//! depended on being in the same address space.

use crate::error::{SdkError, SdkResult};
use crate::identity::{Authority, ContractDecl, ContractId, Event, EventDecl, Version};
use crate::runtime::Runtime;
use crate::value::Value;

/// The link version this host speaks.
pub const LINK_VERSION: Version = Version::new(1, 0, 0);

/// Every link version this host can speak. Peers must agree exactly: a link is
/// a negotiated relationship, and a mismatched pair is refused rather than
/// half-understood.
pub const SUPPORTED_LINK_VERSIONS: &[Version] = &[LINK_VERSION];

/// What a host advertises to a peer while the link is being established.
///
/// Derived from that host's own registry, so a peer discovers what the runtime
/// actually holds rather than what the host claimed it would hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// The host's own identity, as that host declares it. Carried, not
    /// verified: authenticating a peer is a transport concern this layer does
    /// not have, and pretending otherwise would be a guarantee it cannot keep.
    pub id: String,
    /// The host's own version.
    pub version: Version,
    /// Exact contracts the host's plugins declared, at the versions they
    /// declared them.
    pub contracts: Vec<ContractDecl>,
    /// Exact event identities the host's plugins declared a subscription to.
    pub events: Vec<EventDecl>,
    /// The link version this host speaks.
    pub link: Version,
}

impl Peer {
    /// Read a host's surface out of its own registry.
    pub fn advertise(id: impl Into<String>, version: Version, runtime: &Runtime) -> Peer {
        Peer {
            id: id.into(),
            version,
            contracts: runtime.offered_contracts(),
            events: runtime.subscribed_events(),
            link: LINK_VERSION,
        }
    }

    /// Whether this host advertised this contract at exactly this version.
    fn offers_contract(&self, id: &ContractId, version: Version) -> bool {
        self.contracts
            .iter()
            .any(|d| d.id == *id && d.version == version)
    }

    /// Every version of one contract this host did advertise, for the refusal
    /// that says what *was* on offer.
    fn contract_versions(&self, id: &ContractId) -> Vec<String> {
        self.contracts
            .iter()
            .filter(|d| d.id == *id)
            .map(|d| d.version.to_string())
            .collect()
    }

    /// Whether this host declared a subscription to this event at exactly this
    /// version.
    fn subscribes_to(&self, event: &str, version: u32) -> bool {
        self.events
            .iter()
            .any(|e| e.id == event && e.version == version)
    }

    /// Every version of one event this host declared a subscription to.
    fn event_versions(&self, event: &str) -> Vec<String> {
        self.events
            .iter()
            .filter(|e| e.id == event)
            .map(|e| e.version.to_string())
            .collect()
    }
}

/// The standing one host holds over another across a link.
///
/// Three, because a host can say three things about another host without
/// knowing what either of them does: it settles, it proposes, or it looks. What
/// any of those *mean* is the host's own policy and is deliberately not encoded
/// here. This layer carries the standing and enforces the one thing it can
/// decide on its own: a reader holds no grants, so a receiving host refuses
/// every operation from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Its proposals are the ones the receiving host settles.
    Master,
    /// It may submit, and the receiving host judges them as proposals.
    Proposer,
    /// It may hold no grants at all, so it may submit nothing.
    Reader,
}

/// A directed, authorised link from one host to another.
///
/// The local host holds the link and the remote is the host it addresses. Every
/// operation crosses *into* the remote's runtime, and the remote decides:
/// `Link` presents the authority the receiving host chose to grant and lets the
/// receiving host judge it against the capabilities its own plugins declared.
/// This is why a link cannot grant itself anything.
#[derive(Debug)]
pub struct Link {
    local: Peer,
    remote: Peer,
    role: Role,
    grants: Authority,
}

impl Link {
    /// Discovery and handshake: both advertisements are checked, both standings
    /// are recorded, and the link is refused if the two ends disagree.
    ///
    /// Refused when either host's identity is unusable, when a host is handed
    /// itself as its own peer, when the two ends speak different link
    /// versions, when the presented authority is not the local host's own, or
    /// when a reader is handed a grant.
    pub fn open(local: Peer, remote: Peer, role: Role, grants: Authority) -> SdkResult<Link> {
        let refuse = |reason: String| SdkError::InvalidLink {
            host: local.id.clone(),
            reason,
        };

        identity_ok(&local.id, "the local host").map_err(&refuse)?;
        identity_ok(&remote.id, "the peer").map_err(&refuse)?;

        if local.id == remote.id {
            return Err(refuse(format!(
                "host '{}' cannot be its own peer",
                local.id
            )));
        }
        if local.link != remote.link {
            return Err(SdkError::UnsupportedLinkVersion {
                declared: remote.link.to_string(),
                supported: render_supported(),
            });
        }
        if grants.principal != local.id {
            return Err(refuse(format!(
                "the link presents authority '{}' but the local host is '{}'",
                grants.principal, local.id
            )));
        }
        if role == Role::Reader && !grants.granted.is_empty() {
            return Err(refuse(
                "role 'reader' holds no grants, so it cannot be granted one".into(),
            ));
        }

        Ok(Link {
            local,
            remote,
            role,
            grants,
        })
    }

    /// This host, as it advertised itself.
    pub fn local(&self) -> &Peer {
        &self.local
    }

    /// The host this link addresses, as it advertised itself.
    pub fn remote(&self) -> &Peer {
        &self.remote
    }

    /// The standing the remote holds on the local host.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The authority this link presents. Exactly what the receiving host is
    /// asked to judge; never widened here.
    pub fn grants(&self) -> &Authority {
        &self.grants
    }

    /// The link version both ends agreed on.
    pub fn link_version(&self) -> Version {
        self.local.link
    }

    /// Cross the link with a contract, addressed by exact identity and version.
    ///
    /// The version is negotiated against what the peer advertised before
    /// anything is sent, and then the peer's own runtime resolves it against its
    /// own declared capabilities. The refusal for a version the peer never
    /// offered is this layer's; every other refusal is the peer's.
    pub fn invoke(
        &self,
        remote: &Runtime,
        contract: &ContractId,
        version: Version,
        payload: Value,
    ) -> SdkResult<Value> {
        if !self.remote.offers_contract(contract, version) {
            return Err(SdkError::VersionNotOffered {
                peer: self.remote.id.clone(),
                subject: contract.to_string(),
                requested: version.to_string(),
                available: self.remote.contract_versions(contract),
            });
        }
        remote.resolve_contract(&self.grants, contract, version, payload)
    }

    /// Cross the link with a fact. Identity and version are preserved, and an
    /// event the peer never subscribed to at that exact version is refused
    /// rather than quietly dropped.
    pub fn publish(&self, remote: &Runtime, event: &Event) -> SdkResult<()> {
        if !self.remote.subscribes_to(&event.id, event.version) {
            return Err(SdkError::VersionNotOffered {
                peer: self.remote.id.clone(),
                subject: event.id.clone(),
                requested: event.version.to_string(),
                available: self.remote.event_versions(&event.id),
            });
        }
        remote.publish(&self.grants, event)
    }
}

fn identity_ok(id: &str, end: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err(format!("{end} identity must not be empty"));
    }
    if id.contains(char::is_whitespace) {
        return Err(format!("{end} identity '{id}' must not contain whitespace"));
    }
    Ok(())
}

fn render_supported() -> String {
    SUPPORTED_LINK_VERSIONS
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, contract: &str, version: Version) -> Peer {
        Peer {
            id: id.into(),
            version: Version::new(1, 0, 0),
            contracts: vec![ContractDecl::new(contract, version)],
            events: Vec::new(),
            link: LINK_VERSION,
        }
    }

    fn none() -> Authority {
        Authority::new("host-a@atlas", vec![])
    }

    #[test]
    fn a_peer_offers_only_the_version_it_declared() {
        let advertised = peer("host-b@atlas", "relay.compute", Version::new(1, 0, 0));
        let id = ContractId::new("relay.compute");
        assert!(advertised.offers_contract(&id, Version::new(1, 0, 0)));
        // A near version is not an offered version.
        assert!(!advertised.offers_contract(&id, Version::new(1, 0, 1)));
        assert!(!advertised.offers_contract(&id, Version::new(2, 0, 0)));
        assert_eq!(advertised.contract_versions(&id), vec!["1.0.0".to_string()]);
    }

    #[test]
    fn a_host_may_not_be_its_own_peer() {
        let err = Link::open(
            peer("host-a@atlas", "a.thing", Version::new(1, 0, 0)),
            peer("host-a@atlas", "b.thing", Version::new(1, 0, 0)),
            Role::Proposer,
            none(),
        )
        .expect_err("a host cannot link to itself");
        assert_eq!(
            err,
            SdkError::InvalidLink {
                host: "host-a@atlas".into(),
                reason: "host 'host-a@atlas' cannot be its own peer".into(),
            }
        );
    }

    #[test]
    fn an_advertisement_reads_the_registry_not_a_claim() {
        let mut runtime = Runtime::new();
        runtime.discover(crate::default_system());
        let advertised = Peer::advertise("host-a@atlas", Version::new(1, 0, 0), &runtime);
        assert_eq!(
            advertised.contracts,
            vec![ContractDecl::new(
                "inventory.reserve",
                Version::new(1, 0, 0)
            )]
        );
        // `orders` binds its subscription through `Plugin::subscriptions` rather
        // than declaring it, so the *declared* event surface is empty — and the
        // advertisement says so rather than reporting a bound handler.
        assert!(advertised.events.is_empty());

        // A runtime with nothing in it advertises nothing, rather than claiming
        // the surface of some other host.
        let empty = Peer::advertise("host-c@atlas", Version::new(1, 0, 0), &Runtime::new());
        assert!(empty.contracts.is_empty() && empty.events.is_empty());
    }
}
