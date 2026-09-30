//! Plugin implementations for the fundamental-loop proof.
//!
//! One module per plugin, on purpose: it makes "no direct plugin-to-plugin
//! coupling" a property of the file layout, and therefore something a test can
//! actually check.

pub mod inventory;
pub mod orders;
pub mod relay;
