//! Atlas-SDK — a general-purpose application runtime and plugin platform.
//!
//! The SDK owns the orchestration lifecycle. A host supplies environment and
//! plugins; it does not supply a competing lifecycle, registry or contract
//! system.
//!
//! This crate proves the fundamental loop and nothing more:
//!
//! ```text
//! discover -> validate -> register -> initialize -> expose capability
//!   -> resolve contract -> publish/consume event -> execute with context
//!   -> shutdown
//! ```

pub mod context;
pub mod error;
pub mod identity;
pub mod manifest;
pub mod plugin;
pub mod plugins;
pub mod runtime;
pub mod value;

pub use context::Context;
pub use error::{SdkError, SdkResult};
pub use identity::{Authority, Capability, ContractDecl, ContractId, Event, Version};
pub use manifest::{Lifecycle, Manifest};
pub use plugin::Plugin;
pub use runtime::{ExposedCapability, Runtime};
pub use value::Value;

/// The default two-plugin system: one contract provider, one consumer.
pub fn default_system() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(plugins::inventory::Inventory::new()),
        Box::new(plugins::orders::Orders::new()),
    ]
}
