//! `relay` — the plugin the far host of a Connect link serves.
//!
//! It exists so a link has a real receiving host to address: a host that
//! advertises a contract and a subscription, and that hands the link exactly
//! what it sent. The domain-free contract is a request/response that echoes
//! what it was given, because a Connect test is about who crossed the link, not
//! about what the payload turned out to mean.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::{Context, ContractFn};
use crate::error::SdkResult;
use crate::identity::{Capability, ContractDecl, Event, EventDecl, Version};
use crate::manifest::{Lifecycle, Manifest};
use crate::plugin::{subscription, Plugin, Subscription};
use crate::value::Value;

/// The contract this host serves across a link. Declared here and named again
/// by the test as a string, so the two ends agree by identity and not by
/// sharing a symbol.
pub fn compute_decl() -> ContractDecl {
    ContractDecl::new("relay.compute", Version::new(1, 0, 0))
}

/// The event this host listens for across a link.
pub const RELAY_SIGNAL: &str = "relay.signal";

pub fn signal_decl() -> EventDecl {
    EventDecl::new(RELAY_SIGNAL, 1)
}

pub struct Relay {
    manifest: Manifest,
    observed: Rc<RefCell<Vec<String>>>,
}

impl Relay {
    /// `observed` is shared with whoever is running the host, so a test can read
    /// back what actually arrived here instead of inferring it from the other
    /// end's side of the link.
    pub fn new(observed: &Rc<RefCell<Vec<String>>>) -> Self {
        let manifest = Manifest::new("relay", Version::new(1, 0, 0))
            .provides(compute_decl())
            .subscribes(signal_decl())
            .capability(Capability::new("compute:relay", "relay.compute"))
            .lifecycle(Lifecycle::ShutdownAware);
        Relay {
            manifest,
            observed: Rc::clone(observed),
        }
    }

    /// Every call and every fact that reached this host, in order.
    pub fn observed(&self) -> Vec<String> {
        self.observed.borrow().clone()
    }
}

impl Plugin for Relay {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn contracts(&self) -> Vec<(ContractDecl, ContractFn)> {
        let observed = Rc::clone(&self.observed);
        let handler: ContractFn = Rc::new(move |ctx: &Context, payload: Value| {
            // The principal is read from the call, not from the payload: a
            // payload claiming to be somebody is still just a payload.
            let principal = ctx.principal();
            observed.borrow_mut().push(format!(
                "served {} v1.0.0 principal={principal}",
                compute_decl().id
            ));
            Ok(Value::map(vec![
                ("principal", Value::str(principal)),
                ("echo", payload),
            ]))
        });

        vec![(compute_decl(), handler)]
    }

    fn subscriptions(&self) -> Vec<Subscription> {
        let observed = Rc::clone(&self.observed);
        vec![subscription(
            RELAY_SIGNAL,
            1,
            move |ctx: &Context, event: &Event| {
                observed.borrow_mut().push(format!(
                    "heard {} v{} principal={}",
                    event.id,
                    event.version,
                    ctx.principal()
                ));
                ctx.audit(&format!("{} v{} handled", event.id, event.version));
                Ok(())
            },
        )]
    }

    fn on_shutdown(&mut self, ctx: &Context) -> SdkResult<()> {
        ctx.log(&format!(
            "relay closing, {} crossed the link",
            self.observed().len()
        ));
        Ok(())
    }
}
