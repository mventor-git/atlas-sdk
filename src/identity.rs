//! Identity, capability and authority declarations.
//!
//! Authority is data, not a call-stack accident: it is constructed once at the
//! call site, carried in the context, and readable by the callee at the
//! moment of invocation.

use std::fmt;

/// Semver-shaped version, ordered so the registry can compare declared
/// versions instead of assuming the newest one is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A stable contract identity, e.g. `inventory.reserve`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContractId(pub String);

impl ContractId {
    pub fn new(s: impl Into<String>) -> Self {
        ContractId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContractId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A contract as a plugin declares it: identity plus the exact version it
/// provides or requires.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContractDecl {
    pub id: ContractId,
    pub version: Version,
}

impl ContractDecl {
    pub fn new(id: impl Into<String>, version: Version) -> Self {
        ContractDecl {
            id: ContractId::new(id),
            version,
        }
    }
}

/// An event identity plus version. Events are facts, not requests.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventDecl {
    pub id: String,
    pub version: u32,
}

impl EventDecl {
    pub fn new(id: impl Into<String>, version: u32) -> Self {
        EventDecl {
            id: id.into(),
            version,
        }
    }
}

/// Something a plugin can do. Explicit and machine-readable: a host can ask
/// what exists, and the runtime can ask what authority it requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    /// The authority a caller must hold to invoke it.
    pub requires: String,
}

impl Capability {
    pub fn new(name: impl Into<String>, requires: impl Into<String>) -> Self {
        Capability {
            name: name.into(),
            requires: requires.into(),
        }
    }
}

/// Who is calling, and on what authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    pub principal: String,
    /// Capabilities this principal holds.
    pub granted: Vec<String>,
}

impl Authority {
    pub fn new(principal: impl Into<String>, granted: Vec<String>) -> Self {
        Authority {
            principal: principal.into(),
            granted,
        }
    }

    pub fn holds(&self, capability: &str) -> bool {
        self.granted.iter().any(|c| c == capability)
    }
}

/// A concrete fact that occurred, carrying its own version.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub version: u32,
    pub payload: crate::value::Value,
}

impl Event {
    pub fn new(id: impl Into<String>, version: u32, payload: crate::value::Value) -> Self {
        Event {
            id: id.into(),
            version,
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_by_significance() {
        assert!(Version::new(1, 0, 0) < Version::new(1, 2, 0));
        assert!(Version::new(1, 9, 9) < Version::new(2, 0, 0));
        assert_eq!(Version::new(3, 1, 4).to_string(), "3.1.4");
    }

    #[test]
    fn authority_checks_exact_grants() {
        let a = Authority::new("ops@atlas", vec!["inventory.reserve".into()]);
        assert!(a.holds("inventory.reserve"));
        assert!(!a.holds("payroll.read"));
    }
}
