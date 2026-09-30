//! The manifest is everything a plugin declares about itself.
//!
//! Nothing else about a plugin is knowable before it runs. The registry
//! validates manifests, and only manifests, during `validate`.

use crate::identity::{Capability, ContractDecl, EventDecl};

/// What the plugin wants the SDK to do for it, expressed as lifecycle
/// behaviour rather than as code the host has to special-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lifecycle {
    /// Wants no callbacks; a pure capability provider.
    Passive,
    /// Wants `on_initialize` with a declared ordering weight. Lower runs first.
    Ordered(i32),
    /// Wants `on_shutdown` invoked.
    ShutdownAware,
}

impl Lifecycle {
    pub fn initialize_order(&self) -> i32 {
        match self {
            Lifecycle::Ordered(n) => *n,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Stable identity. Duplicates are refused.
    pub id: String,
    pub version: crate::identity::Version,
    /// Exact contract identities and versions this plugin implements.
    pub contracts_provided: Vec<ContractDecl>,
    /// Exact contracts this plugin needs from others.
    pub contracts_required: Vec<ContractDecl>,
    /// Events this plugin reacts to. Declared, not discovered, so a host can
    /// reason about the running system before anything fires.
    pub subscriptions: Vec<EventDecl>,
    /// Explicit, machine-readable capabilities.
    pub capabilities: Vec<Capability>,
    pub lifecycle: Lifecycle,
}

impl Manifest {
    pub fn new(id: impl Into<String>, version: crate::identity::Version) -> Self {
        Manifest {
            id: id.into(),
            version,
            contracts_provided: Vec::new(),
            contracts_required: Vec::new(),
            subscriptions: Vec::new(),
            capabilities: Vec::new(),
            lifecycle: Lifecycle::Passive,
        }
    }

    pub fn provides(mut self, decl: ContractDecl) -> Self {
        self.contracts_provided.push(decl);
        self
    }

    pub fn requires(mut self, decl: ContractDecl) -> Self {
        self.contracts_required.push(decl);
        self
    }

    pub fn subscribes(mut self, decl: EventDecl) -> Self {
        self.subscriptions.push(decl);
        self
    }

    pub fn capability(mut self, cap: Capability) -> Self {
        self.capabilities.push(cap);
        self
    }

    pub fn lifecycle(mut self, l: Lifecycle) -> Self {
        self.lifecycle = l;
        self
    }

    /// Structural checks that need no knowledge of other plugins.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("identity must not be empty".into());
        }
        if self.id.contains(char::is_whitespace) {
            return Err(format!(
                "identity '{}' must not contain whitespace",
                self.id
            ));
        }
        if self.contracts_provided.is_empty() && self.contracts_required.is_empty() {
            return Err("manifest declares neither contracts provided nor required".into());
        }
        for cap in &self.capabilities {
            if cap.name.trim().is_empty() || cap.requires.trim().is_empty() {
                return Err("capability name and required authority must both be non-empty".into());
            }
        }
        for event in &self.subscriptions {
            if event.id.trim().is_empty() {
                return Err("subscribed event identity must not be empty".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Version;

    #[test]
    fn rejects_empty_identity() {
        let m = Manifest::new("   ", Version::new(1, 0, 0))
            .provides(ContractDecl::new("x.y", Version::new(1, 0, 0)));
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_manifest_that_declares_nothing() {
        let m = Manifest::new("empty.plugin", Version::new(1, 0, 0));
        assert!(m.validate().is_err());
    }

    #[test]
    fn accepts_a_real_manifest() {
        let m = Manifest::new("alpha", Version::new(1, 0, 0))
            .requires(ContractDecl::new(
                "inventory.reserve",
                Version::new(1, 0, 0),
            ))
            .capability(Capability::new("order.submit", "order.submit"));
        assert!(m.validate().is_ok());
    }

    #[test]
    fn rejects_an_empty_subscribed_event_identity() {
        let m = Manifest::new("alpha", Version::new(1, 0, 0))
            .provides(ContractDecl::new("x.y", Version::new(1, 0, 0)))
            .subscribes(crate::identity::EventDecl::new("  ", 1));
        assert!(m.validate().is_err());
    }
}
