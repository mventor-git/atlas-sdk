//! The plugin boundary.
//!
//! A plugin is: a manifest it declares up front, a set of contract handlers
//! keyed by identity, a set of event subscriptions, and lifecycle callbacks.
//!
//! Critically, a plugin never receives a handle to another plugin. Everything
//! it can reach is inside `Context`, and everything in `Context` is an SDK
//! port. That is the whole enforcement mechanism for "no direct coupling" —
//! there is no API through which coupling could be expressed.

use std::rc::Rc;

use crate::context::{Context, ContractFn, EventFn};
use crate::error::SdkResult;
use crate::identity::EventDecl;
use crate::manifest::Manifest;

/// An event subscription: the event it wants, and what to do when it fires.
pub struct Subscription {
    pub event: EventDecl,
    pub handler: EventFn,
}

pub trait Plugin {
    /// Everything the registry needs to know before running anything.
    fn manifest(&self) -> &Manifest;

    /// Contract implementations, keyed by identity + version.
    fn contracts(&self) -> Vec<(crate::identity::ContractDecl, ContractFn)> {
        Vec::new()
    }

    /// Events this plugin reacts to.
    fn subscriptions(&self) -> Vec<Subscription> {
        Vec::new()
    }

    /// SDK-driven startup. Receives a context port, never the host.
    fn on_initialize(&mut self, ctx: &Context) -> SdkResult<()> {
        let _ = ctx;
        Ok(())
    }

    /// SDK-driven shutdown, invoked in reverse dependency order.
    fn on_shutdown(&mut self, _ctx: &Context) -> SdkResult<()> {
        Ok(())
    }
}

/// Convenience for building a subscription.
pub fn subscription(
    id: impl Into<String>,
    version: u32,
    handler: impl Fn(&Context, &crate::identity::Event) -> SdkResult<()> + 'static,
) -> Subscription {
    Subscription {
        event: EventDecl::new(id, version),
        handler: Rc::new(handler),
    }
}
