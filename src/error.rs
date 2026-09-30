//! Error type for every deterministic SDK refusal.
//!
//! Every variant renders to a stable, assertable string. Determinism is part
//! of the contract: the same bad input must always produce the same message,
//! because the registry is required to be observable and reproducible.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkError {
    /// A manifest is structurally invalid.
    InvalidManifest { plugin: String, reason: String },
    /// Two plugins declared the same identity. The second is never started.
    DuplicatePluginId {
        id: String,
        existing: String,
        incoming: String,
    },
    /// A plugin requires a contract that nobody provides.
    MissingContract {
        plugin: String,
        contract: String,
        version: String,
    },
    /// The contract exists but not at a version the runtime will accept.
    /// Refused, never silently coerced.
    UnsupportedContractVersion {
        plugin: String,
        contract: String,
        requested: String,
        available: Vec<String>,
    },
    /// Contract dependencies between plugins form a cycle, so no shutdown
    /// order exists.
    DependencyCycle { plugins: Vec<String> },
    /// A cluster declaration is structurally invalid.
    InvalidCluster { cluster: String, reason: String },
    /// Two clusters declared the same identity. The second is refused.
    DuplicateClusterId { id: String },
    /// A cluster names a plugin identity that was never discovered.
    UnknownClusterMember { cluster: String, member: String },
    /// A relationship the cluster declared does not hold against the manifests.
    UnmetClusterRelation { cluster: String, detail: String },
    /// A caller invoked a capability it does not hold authority for.
    Unauthorized {
        principal: String,
        capability: String,
    },
    /// The contract identity is not registered at all.
    ContractNotFound { contract: String, version: String },
    /// An event was published with a version nobody subscribed to.
    NoSubscribers { event: String, version: u32 },
    /// A plugin's own handler returned a failure.
    HandlerFailed { plugin: String, reason: String },
    /// A lifecycle step ran in a state that does not allow it.
    IllegalState { step: String, detail: String },
    /// A plugin spoke the protocol incorrectly.
    ProtocolViolation { detail: String },
    /// A plugin declared a protocol version this runtime does not support.
    UnsupportedProtocolVersion { declared: String, supported: String },
    /// A foreign plugin process could not be started.
    ForeignPluginSpawn { command: String, reason: String },
    /// A foreign plugin failed to answer within the deadline.
    PluginTimeout { command: String, millis: u64 },
}

impl fmt::Display for SdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SdkError::InvalidManifest { plugin, reason } => {
                write!(f, "invalid manifest for plugin '{plugin}': {reason}")
            }
            SdkError::DuplicatePluginId { id, existing, incoming } => write!(
                f,
                "duplicate plugin identity '{id}': already registered by '{existing}', rejected '{incoming}'"
            ),
            SdkError::MissingContract { plugin, contract, version } => write!(
                f,
                "plugin '{plugin}' requires contract '{contract}' v{version} which no registered plugin provides"
            ),
            SdkError::UnsupportedContractVersion {
                plugin,
                contract,
                requested,
                available,
            } => write!(
                f,
                "plugin '{plugin}' requires contract '{contract}' v{requested}, unavailable (available: [{}])",
                available.join(", ")
            ),
            SdkError::DependencyCycle { plugins } => {
                write!(f, "dependency cycle between plugins: {}", plugins.join(" -> "))
            }
            SdkError::InvalidCluster { cluster, reason } => {
                write!(f, "invalid cluster '{cluster}': {reason}")
            }
            SdkError::DuplicateClusterId { id } => {
                write!(f, "duplicate cluster identity '{id}': already declared, rejected the second")
            }
            SdkError::UnknownClusterMember { cluster, member } => write!(
                f,
                "cluster '{cluster}' names member '{member}', which was never discovered"
            ),
            SdkError::UnmetClusterRelation { cluster, detail } => {
                write!(f, "cluster '{cluster}' relationship unmet: {detail}")
            }
            SdkError::Unauthorized { principal, capability } => write!(
                f,
                "principal '{principal}' is not authorized for capability '{capability}'"
            ),
            SdkError::ContractNotFound { contract, version } => {
                write!(f, "contract '{contract}' v{version} is not registered")
            }
            SdkError::NoSubscribers { event, version } => {
                write!(f, "event '{event}' v{version} has no subscribers")
            }
            SdkError::HandlerFailed { plugin, reason } => {
                write!(f, "plugin '{plugin}' handler failed: {reason}")
            }
            SdkError::IllegalState { step, detail } => {
                write!(f, "lifecycle step '{step}' not permitted: {detail}")
            }
            SdkError::ProtocolViolation { detail } => {
                write!(f, "plugin protocol violation: {detail}")
            }
            SdkError::UnsupportedProtocolVersion { declared, supported } => write!(
                f,
                "plugin speaks protocol version {declared} which this runtime does not support (supported: {supported})"
            ),
            SdkError::ForeignPluginSpawn { command, reason } => {
                write!(f, "could not start foreign plugin '{command}': {reason}")
            }
            SdkError::PluginTimeout { command, millis } => {
                write!(f, "foreign plugin '{command}' did not answer within {millis}ms")
            }
        }
    }
}

impl std::error::Error for SdkError {}

pub type SdkResult<T> = Result<T, SdkError>;
