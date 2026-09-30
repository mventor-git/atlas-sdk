//! A cluster: a declaration that groups related plugins.
//!
//! A cluster is metadata, not an actor. It has no lifecycle, no callbacks and
//! no handle on the plugins inside it. `Plugin` has no cluster method and
//! `Manifest` has no cluster field, so a plugin cannot learn that it was
//! grouped and its behaviour is identical either way. Only `Runtime::validate`
//! reads a cluster, which is why an invalid grouping is refused while the
//! system is still inert.
//!
//! Relationships are declared, not inferred. `SharedCapability` asserts that
//! named members really do declare one capability in common; `DependsOn`
//! asserts that a member's own requirement on a contract is closed over inside
//! the group. Both are checked against the manifests the registry actually
//! holds, so a cluster is a claim about the running system that can be refused
//! — not a comment about it.

use std::collections::BTreeSet;

use crate::identity::{ContractDecl, Version};

/// A relationship declared over the members of one cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterRelation {
    /// Every named member declares this capability in its own manifest.
    SharedCapability {
        capability: String,
        members: Vec<String>,
    },
    /// `member` requires `contract`, and a member of this same cluster provides
    /// it: the group is closed over that dependency.
    DependsOn {
        member: String,
        contract: ContractDecl,
    },
}

impl ClusterRelation {
    pub fn shared_capability(capability: impl Into<String>, members: &[&str]) -> Self {
        ClusterRelation::SharedCapability {
            capability: capability.into(),
            members: members.iter().map(|m| (*m).to_string()).collect(),
        }
    }

    pub fn depends_on(member: impl Into<String>, contract: ContractDecl) -> Self {
        ClusterRelation::DependsOn {
            member: member.into(),
            contract,
        }
    }
}

/// A named group of plugin identities, plus the relationships declared over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    /// Stable identity. Duplicates are refused.
    pub id: String,
    pub version: Version,
    /// Plugin identities in this cluster, in declaration order.
    pub members: Vec<String>,
    pub relations: Vec<ClusterRelation>,
}

impl Cluster {
    pub fn new(id: impl Into<String>, version: Version) -> Self {
        Cluster {
            id: id.into(),
            version,
            members: Vec::new(),
            relations: Vec::new(),
        }
    }

    pub fn member(mut self, id: impl Into<String>) -> Self {
        self.members.push(id.into());
        self
    }

    pub fn relation(mut self, relation: ClusterRelation) -> Self {
        self.relations.push(relation);
        self
    }

    /// Whether this cluster declares the given plugin. Observation only — a
    /// plugin has no way to ask.
    pub fn contains(&self, plugin: &str) -> bool {
        self.members.iter().any(|m| m.as_str() == plugin)
    }

    /// Structural checks that need no knowledge of the registry. Whether the
    /// relationships actually hold is `Runtime::validate`'s question, because
    /// only it can see the manifests.
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
        if self.members.is_empty() {
            return Err("cluster declares no members".into());
        }

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for member in &self.members {
            if member.trim().is_empty() {
                return Err("member identity must not be empty".into());
            }
            if !seen.insert(member.as_str()) {
                return Err(format!("member '{member}' is listed more than once"));
            }
        }

        for relation in &self.relations {
            match relation {
                ClusterRelation::SharedCapability {
                    capability,
                    members,
                } => {
                    if capability.trim().is_empty() {
                        return Err("shared capability name must not be empty".into());
                    }
                    if members.is_empty() {
                        return Err(format!("shared capability '{capability}' names no members"));
                    }
                }
                ClusterRelation::DependsOn { contract, .. } => {
                    if contract.id.as_str().trim().is_empty() {
                        return Err("declared dependency names no contract".into());
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> Cluster {
        Cluster::new("commerce", Version::new(1, 0, 0))
            .member("provider")
            .member("consumer")
            .relation(ClusterRelation::shared_capability(
                "shared:thing",
                &["provider", "consumer"],
            ))
    }

    #[test]
    fn accepts_a_real_cluster() {
        assert!(real().validate().is_ok());
    }

    #[test]
    fn rejects_a_cluster_that_groups_nothing() {
        let c = Cluster::new("empty", Version::new(1, 0, 0));
        assert_eq!(c.validate().unwrap_err(), "cluster declares no members");
    }

    #[test]
    fn rejects_a_member_listed_twice() {
        let c = Cluster::new("dupes", Version::new(1, 0, 0))
            .member("a.one")
            .member("a.one");
        assert_eq!(
            c.validate().unwrap_err(),
            "member 'a.one' is listed more than once"
        );
    }

    #[test]
    fn reports_membership() {
        assert!(real().contains("provider"));
        assert!(!real().contains("stranger"));
    }
}
