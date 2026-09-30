//! The runtime owns the orchestration lifecycle.
//!
//! This is the enforcement point for the whole architecture: a host hands
//! plugins to the runtime and gets a composed system back. The host cannot
//! skip `validate`, cannot start a rejected plugin, and cannot observe a
//! shutdown order other than the one the runtime computes.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::cluster::{Cluster, ClusterRelation};
use crate::context::{Context, Platform};
use crate::error::{SdkError, SdkResult};
use crate::identity::{Authority, ContractDecl, ContractId, Event, EventDecl, Version};
use crate::manifest::Manifest;
use crate::plugin::Plugin;
use crate::value::Value;

/// A plugin plus the bookkeeping the runtime needs to order it.
struct Entry {
    manifest: Manifest,
    plugin: Box<dyn Plugin>,
}

/// Public, read-only view of what the system exposes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExposedCapability {
    pub plugin: String,
    pub name: String,
    pub requires: String,
}

pub struct Runtime {
    entries: Vec<Entry>,
    clusters: Vec<Cluster>,
    platform: Rc<Platform>,
    shutdown_order: Vec<String>,
    is_shutdown: bool,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            entries: Vec::new(),
            clusters: Vec::new(),
            platform: Rc::new(Platform::default()),
            shutdown_order: Vec::new(),
            is_shutdown: false,
        }
    }

    /// A runtime preloaded with host settings. The host supplies environment;
    /// it does not get to supply orchestration rules.
    pub fn with_config(pairs: &[(&str, &str)]) -> Self {
        let platform = Platform::default();
        for (k, v) in pairs {
            platform.set_config(k, v);
        }
        Runtime {
            entries: Vec::new(),
            clusters: Vec::new(),
            platform: Rc::new(platform),
            shutdown_order: Vec::new(),
            is_shutdown: false,
        }
    }

    // ---- 1. discover -------------------------------------------------------

    /// Accept plugins from a host. Order is preserved as given; every later
    /// step is deterministic with respect to it.
    pub fn discover(&mut self, plugins: Vec<Box<dyn Plugin>>) -> &mut Self {
        for plugin in plugins {
            let manifest = plugin.manifest().clone();
            self.entries.push(Entry { manifest, plugin });
        }
        self
    }

    /// Group already-discovered plugins under a name. Metadata only: no plugin
    /// learns it was grouped, and nothing below reads a cluster except
    /// `validate`. Declaration order is preserved for observation.
    pub fn declare_cluster(&mut self, cluster: Cluster) -> &mut Self {
        self.clusters.push(cluster);
        self
    }

    // ---- 2. validate -------------------------------------------------------

    /// Validate every manifest and every cross-plugin requirement, in order.
    /// Nothing is started until all of it passes.
    pub fn validate(&mut self) -> SdkResult<()> {
        for entry in &self.entries {
            entry
                .manifest
                .validate()
                .map_err(|reason| SdkError::InvalidManifest {
                    plugin: entry.manifest.id.clone(),
                    reason,
                })?;
        }

        // Duplicate identity: first registration wins, the later one is
        // refused deterministically and never starts.
        let mut first_seen: BTreeMap<String, ()> = BTreeMap::new();
        for entry in &self.entries {
            if first_seen.contains_key(&entry.manifest.id) {
                return Err(SdkError::DuplicatePluginId {
                    id: entry.manifest.id.clone(),
                    existing: entry.manifest.id.clone(),
                    incoming: entry.manifest.id.clone(),
                });
            }
            first_seen.insert(entry.manifest.id.clone(), ());
        }

        // Two plugins may not provide the same contract at the same version.
        let mut providers: BTreeMap<ContractDecl, String> = BTreeMap::new();
        for entry in &self.entries {
            for decl in &entry.manifest.contracts_provided {
                if let Some(existing) = providers.get(decl) {
                    if existing != &entry.manifest.id {
                        return Err(SdkError::DuplicatePluginId {
                            id: format!("{} v{}", decl.id, decl.version),
                            existing: existing.clone(),
                            incoming: entry.manifest.id.clone(),
                        });
                    }
                }
                providers.insert(decl.clone(), entry.manifest.id.clone());
            }
        }

        // Required contracts must exist at the exact declared version. A
        // version mismatch is refused, never silently coerced.
        for entry in &self.entries {
            for required in &entry.manifest.contracts_required {
                if providers.contains_key(required) {
                    continue;
                }
                let available: Vec<String> = providers
                    .keys()
                    .filter(|d| d.id == required.id)
                    .map(|d| d.version.to_string())
                    .collect();
                if available.is_empty() {
                    return Err(SdkError::MissingContract {
                        plugin: entry.manifest.id.clone(),
                        contract: required.id.to_string(),
                        version: required.version.to_string(),
                    });
                }
                return Err(SdkError::UnsupportedContractVersion {
                    plugin: entry.manifest.id.clone(),
                    contract: required.id.to_string(),
                    requested: required.version.to_string(),
                    available,
                });
            }
        }

        // A cluster is a claim about the plugins above, checked in the same
        // pass so an invalid grouping is refused before anything registers or
        // starts.
        let mut cluster_ids: BTreeMap<String, ()> = BTreeMap::new();
        for cluster in &self.clusters {
            if cluster_ids.contains_key(&cluster.id) {
                return Err(SdkError::DuplicateClusterId {
                    id: cluster.id.clone(),
                });
            }
            cluster_ids.insert(cluster.id.clone(), ());
            validate_cluster(cluster, &self.entries, &providers)?;
        }

        // A shutdown order must exist, which also means no dependency cycle.
        self.shutdown_order = compute_shutdown_order(&self.entries, &providers)?;
        Ok(())
    }

    // ---- 3. register -------------------------------------------------------

    /// Bind declared contracts and subscriptions into the platform, so a
    /// contract is resolvable by identity and never by object reference.
    pub fn register(&mut self) -> SdkResult<()> {
        for entry in &self.entries {
            for (decl, handler) in entry.plugin.contracts() {
                self.platform.register_contract(
                    entry.manifest.id.clone(),
                    (decl.id.clone(), decl.version),
                    handler,
                );
            }
            for sub in entry.plugin.subscriptions() {
                self.platform.register_subscription(
                    entry.manifest.id.clone(),
                    sub.event.id.clone(),
                    sub.event.version,
                    sub.handler,
                );
            }
        }
        Ok(())
    }

    // ---- 4. initialize -----------------------------------------------------

    /// Run each plugin's startup in declared lifecycle order, then discovery
    /// order. A failure stops here; later plugins are not started.
    pub fn initialize(&mut self) -> SdkResult<()> {
        if self.is_shutdown {
            return Err(SdkError::IllegalState {
                step: "initialize".into(),
                detail: "runtime is already shut down".into(),
            });
        }

        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|i| (self.entries[*i].manifest.lifecycle.initialize_order(), *i));

        for index in order {
            let ctx = Context::new(host_authority(), Rc::clone(&self.platform));
            let id = self.entries[index].manifest.id.clone();
            self.entries[index]
                .plugin
                .on_initialize(&ctx)
                .map_err(|e| SdkError::HandlerFailed {
                    plugin: id,
                    reason: e.to_string(),
                })?;
        }
        Ok(())
    }

    // ---- 5. expose capability ---------------------------------------------

    /// Publish the machine-readable capability surface.
    pub fn expose_capabilities(&self) -> Vec<ExposedCapability> {
        let mut out = Vec::new();
        for entry in &self.entries {
            for cap in &entry.manifest.capabilities {
                out.push(ExposedCapability {
                    plugin: entry.manifest.id.clone(),
                    name: cap.name.clone(),
                    requires: cap.requires.clone(),
                });
            }
        }
        out
    }

    // ---- 6. resolve contract ----------------------------------------------

    /// Resolve a contract by identity for a caller holding the required
    /// authority. This is the only cross-plugin path in the system.
    pub fn resolve_contract(
        &self,
        caller: &Authority,
        id: &ContractId,
        version: Version,
        payload: Value,
    ) -> SdkResult<Value> {
        let required =
            self.required_authority_for(id, version)
                .ok_or_else(|| SdkError::ContractNotFound {
                    contract: id.to_string(),
                    version: version.to_string(),
                })?;

        if !caller.holds(&required) {
            return Err(SdkError::Unauthorized {
                principal: caller.principal.clone(),
                capability: required,
            });
        }

        let ctx = Context::new(caller.clone(), Rc::clone(&self.platform));
        ctx.call(id, version, payload)
    }

    /// The authority a caller must hold to invoke this contract: the `requires`
    /// field of the capability declared alongside it.
    ///
    /// Returns `None` when the provider declared no such capability, and there is
    /// no fallback. `Manifest::validate` refuses such a manifest, so this is
    /// unreachable through the normal lifecycle; if `validate` is skipped, the
    /// correct answer is still to refuse. Naming the contract after itself would
    /// be inventing an authorization rule the plugin never asked for, and would
    /// hand the contract to anyone who guessed its name.
    fn required_authority_for(&self, id: &ContractId, version: Version) -> Option<String> {
        self.entries.iter().find_map(|entry| {
            entry
                .manifest
                .contracts_provided
                .iter()
                .find(|d| d.id == *id && d.version == version)
                .and_then(|d| {
                    entry
                        .manifest
                        .capabilities
                        .iter()
                        .find(|c| c.requires == d.id.to_string())
                        .map(|c| c.requires.clone())
                })
        })
    }

    // ---- 7. publish / consume event ---------------------------------------

    pub fn publish(&self, publisher: &Authority, event: &Event) -> SdkResult<()> {
        let ctx = Context::new(publisher.clone(), Rc::clone(&self.platform));
        ctx.publish(event)
    }

    // ---- 8. execute with context ------------------------------------------

    pub fn audit_records(&self) -> Vec<String> {
        self.platform.audit_records()
    }

    pub fn log_records(&self) -> Vec<String> {
        self.platform.log_records()
    }

    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown
    }

    // ---- 9. shutdown -------------------------------------------------------

    /// Shut down in reverse dependency order. Idempotent: calling it again is a
    /// no-op and never double-shuts a plugin down.
    pub fn shutdown(&mut self) -> SdkResult<Vec<String>> {
        if self.is_shutdown {
            return Ok(Vec::new());
        }
        self.is_shutdown = true;

        let order = self.shutdown_order.clone();
        for id in &order {
            let index = self
                .entries
                .iter()
                .position(|e| &e.manifest.id == id)
                .ok_or_else(|| SdkError::IllegalState {
                    step: "shutdown".into(),
                    detail: format!("plugin '{id}' vanished before shutdown"),
                })?;
            let ctx = Context::new(host_authority(), Rc::clone(&self.platform));
            self.entries[index]
                .plugin
                .on_shutdown(&ctx)
                .map_err(|e| SdkError::HandlerFailed {
                    plugin: id.clone(),
                    reason: e.to_string(),
                })?;
        }
        Ok(order)
    }

    // ---- observation -------------------------------------------------------

    pub fn plugin_ids(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.manifest.id.clone()).collect()
    }

    /// Declared clusters, in declaration order.
    pub fn clusters(&self) -> Vec<String> {
        self.clusters.iter().map(|c| c.id.clone()).collect()
    }

    /// The cluster a plugin was declared in, or `None` when it is ungrouped.
    /// Observation only: a plugin cannot reach this, which is what keeps
    /// grouping from becoming a behaviour.
    pub fn cluster_of(&self, plugin: &str) -> Option<String> {
        self.clusters
            .iter()
            .find(|c| c.contains(plugin))
            .map(|c| c.id.clone())
    }

    pub fn contracts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.entries {
            for decl in &entry.manifest.contracts_provided {
                out.push(format!("{} v{}", decl.id, decl.version));
            }
        }
        out
    }

    /// Exact contract identities and versions this runtime offers, in discovery
    /// order. Derived from the manifests, so a Connect peer discovers what the
    /// registry actually holds rather than what a host claimed it would hold.
    /// Observation only.
    pub fn offered_contracts(&self) -> Vec<ContractDecl> {
        self.entries
            .iter()
            .flat_map(|e| e.manifest.contracts_provided.iter().cloned())
            .collect()
    }

    /// Exact event identities the registered plugins declared, in discovery
    /// order. The event half of `offered_contracts`, and observation only.
    pub fn subscribed_events(&self) -> Vec<EventDecl> {
        self.entries
            .iter()
            .flat_map(|e| e.manifest.subscriptions.iter().cloned())
            .collect()
    }
}

/// The authority a host-level action runs under. A real host derives this from
/// authentication; the proof only needs it explicit and inspectable.
fn host_authority() -> Authority {
    Authority::new("host@atlas", vec!["system.manage".into()])
}

/// A cluster is validated twice: structurally by `Cluster::validate`, then
/// against the manifests the registry actually holds. The second half is what
/// makes a declared relationship a checkable claim rather than a label, and it
/// is the only place a cluster is consulted — nothing in `register`,
/// `initialize` or `shutdown` reads one, which is why grouping cannot change
/// how a plugin behaves.
fn validate_cluster(
    cluster: &Cluster,
    entries: &[Entry],
    providers: &BTreeMap<ContractDecl, String>,
) -> SdkResult<()> {
    cluster
        .validate()
        .map_err(|reason| SdkError::InvalidCluster {
            cluster: cluster.id.clone(),
            reason,
        })?;

    let is_member = |id: &str| cluster.members.iter().any(|m| m.as_str() == id);

    for member in &cluster.members {
        if !entries.iter().any(|e| &e.manifest.id == member) {
            return Err(SdkError::UnknownClusterMember {
                cluster: cluster.id.clone(),
                member: member.clone(),
            });
        }
    }

    for relation in &cluster.relations {
        let unmet = |detail: String| SdkError::UnmetClusterRelation {
            cluster: cluster.id.clone(),
            detail,
        };

        match relation {
            ClusterRelation::SharedCapability {
                capability,
                members,
            } => {
                for member in members {
                    if !is_member(member) {
                        return Err(unmet(format!(
                            "shared capability '{capability}' names '{member}', which is not a member of this cluster"
                        )));
                    }
                    let declares = entries.iter().any(|e| {
                        e.manifest.id == *member
                            && e.manifest
                                .capabilities
                                .iter()
                                .any(|c| c.name == *capability)
                    });
                    if !declares {
                        return Err(unmet(format!(
                            "shared capability '{capability}' is not declared by member '{member}'"
                        )));
                    }
                }
            }
            ClusterRelation::DependsOn { member, contract } => {
                if !is_member(member) {
                    return Err(unmet(format!(
                        "declared dependency names '{member}', which is not a member of this cluster"
                    )));
                }
                let requires = entries.iter().any(|e| {
                    e.manifest.id == *member && e.manifest.contracts_required.contains(contract)
                });
                if !requires {
                    return Err(unmet(format!(
                        "member '{member}' does not require contract '{}' v{}",
                        contract.id, contract.version
                    )));
                }
                let Some(provider) = providers.get(contract) else {
                    return Err(unmet(format!(
                        "contract '{}' v{} is required by '{member}' but no registered plugin provides it",
                        contract.id, contract.version
                    )));
                };
                if !is_member(provider) {
                    return Err(unmet(format!(
                        "contract '{}' v{} is required by '{member}' but provided by '{provider}', outside this cluster",
                        contract.id, contract.version
                    )));
                }
            }
        }
    }
    Ok(())
}

/// A consumer must shut down before the provider it depends on.
///
/// Ordering is by dependency depth, descending, with discovery order as the
/// tie-break. Depth rather than a topological sort plus a blanket reverse,
/// because siblings with no dependency between them should keep the order they
/// were discovered in instead of being flipped for no reason. Ties are broken
/// explicitly so the result is reproducible across runs.
fn compute_shutdown_order(
    entries: &[Entry],
    providers: &BTreeMap<ContractDecl, String>,
) -> SdkResult<Vec<String>> {
    // dependencies[id] = providers this plugin's contract requirements point at.
    let mut dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in entries {
        let deps: BTreeSet<String> = entry
            .manifest
            .contracts_required
            .iter()
            .filter_map(|d| providers.get(d))
            .filter(|p| *p != &entry.manifest.id)
            .cloned()
            .collect();
        dependencies.insert(entry.manifest.id.clone(), deps);
    }

    // depth(id) = 0 when independent, else 1 + max(depth(dependencies)).
    // Iterative so a cycle is detected rather than overflowing the stack.
    let mut depth: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        let id = entry.manifest.id.clone();
        let mut chain: Vec<String> = Vec::new();
        let mut current = id.clone();

        loop {
            if let Some(d) = depth.get(&current).copied() {
                // Walk back up the chain, each level one deeper than its
                // dependency, so `chain` is in descending depth order.
                for (offset, node) in chain.iter().enumerate() {
                    depth.insert(node.clone(), d + 1 + offset);
                }
                break;
            }
            if chain.contains(&current) {
                return Err(SdkError::DependencyCycle {
                    plugins: chain.to_vec(),
                });
            }
            chain.push(current.clone());

            let Some(deps) = dependencies.get(&current) else {
                for node in &chain {
                    depth.insert(node.clone(), 0);
                }
                break;
            };
            // Deepest dependency sets the depth; walk the deepest first.
            match deps.iter().next() {
                Some(next) => current = next.clone(),
                None => {
                    for node in &chain {
                        depth.insert(node.clone(), 0);
                    }
                    break;
                }
            }
        }
    }

    let mut ordered: Vec<(usize, usize, &str)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (depth[&e.manifest.id], i, e.manifest.id.as_str()))
        .collect();
    ordered.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    Ok(ordered
        .into_iter()
        .map(|(_, _, id)| id.to_string())
        .collect())
}
