//! The runtime owns the orchestration lifecycle.
//!
//! This is the enforcement point for the whole architecture: a host hands
//! plugins to the runtime and gets a composed system back. The host cannot
//! skip `validate`, cannot start a rejected plugin, and cannot observe a
//! shutdown order other than the one the runtime computes.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::context::{Context, Platform};
use crate::error::{SdkError, SdkResult};
use crate::identity::{Authority, ContractDecl, ContractId, Event, Version};
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

    pub fn contracts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.entries {
            for decl in &entry.manifest.contracts_provided {
                out.push(format!("{} v{}", decl.id, decl.version));
            }
        }
        out
    }
}

/// The authority a host-level action runs under. A real host derives this from
/// authentication; the proof only needs it explicit and inspectable.
fn host_authority() -> Authority {
    Authority::new("host@atlas", vec!["system.manage".into()])
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
