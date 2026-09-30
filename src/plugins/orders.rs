//! `orders` — the contract consumer and event subscriber.
//!
//! It never imports the provider. It names a contract identity and an event
//! identity, and the runtime resolves both. `src/plugins/inventory.rs` is not
//! referenced anywhere in this file, and `tests/fundamental_loop.rs` fails if
//! that ever becomes true.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::Context;
use crate::error::SdkResult;
use crate::identity::{Authority, Capability, ContractDecl, Event, Version};
use crate::manifest::{Lifecycle, Manifest};
use crate::plugin::{subscription, Plugin, Subscription};
use crate::value::Value;

/// The contract this plugin requires. It declares the same identity string the
/// provider declares. There is no shared constant between the two modules — if
/// there were, the no-coupling guarantee would be worth nothing.
pub fn reserve_decl() -> ContractDecl {
    ContractDecl::new("inventory.reserve", Version::new(1, 0, 0))
}

pub const ORDER_PLACED: &str = "order.placed";

pub struct Orders {
    manifest: Manifest,
    seen: Rc<RefCell<Vec<String>>>,
}

impl Orders {
    pub fn new() -> Self {
        let manifest = Manifest::new("orders", Version::new(1, 2, 0))
            .requires(reserve_decl())
            .capability(Capability::new("place:order", "orders.place"))
            .lifecycle(Lifecycle::Ordered(10));
        Orders {
            manifest,
            seen: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn seen(&self) -> Vec<String> {
        self.seen.borrow().clone()
    }
}

impl Default for Orders {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Orders {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn subscriptions(&self) -> Vec<Subscription> {
        let seen = Rc::clone(&self.seen);
        vec![subscription(
            ORDER_PLACED,
            1,
            move |ctx: &Context, event: &Event| {
                // The event is a fact. What it *means* is this plugin's business,
                // not the SDK's — the SDK only delivered it.
                let order_id = event
                    .payload
                    .get("order_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();

                // Resolving the provider's contract by identity, through the port.
                // No import, no object reference, no knowledge of the provider.
                let response = ctx.call(
                    &reserve_decl().id,
                    reserve_decl().version,
                    Value::map(vec![
                        ("quantity", Value::num(1)),
                        ("for", Value::str(order_id.clone())),
                    ]),
                )?;

                seen.borrow_mut().push(format!(
                    "{order_id}:{}",
                    response
                        .get("principal")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                ));
                ctx.audit(&format!("order {order_id} fulfilled"));
                Ok(())
            },
        )]
    }

    fn on_initialize(&mut self, ctx: &Context) -> SdkResult<()> {
        // Proves the plugin can read host configuration through the port and
        // never needs the host object itself.
        let currency = ctx.config("currency").unwrap_or_else(|| "unset".into());
        ctx.log(&format!("orders starting, currency={currency}"));
        Ok(())
    }

    fn on_shutdown(&mut self, ctx: &Context) -> SdkResult<()> {
        ctx.log(&format!(
            "orders closing, {} events handled",
            self.seen().len()
        ));
        Ok(())
    }
}

/// Demonstrates that the caller chooses the authority; the runtime checks it
/// against what the provider declared.
pub fn ops_authority() -> Authority {
    Authority::new("ops@atlas", vec!["inventory.reserve".into()])
}

pub fn intern_authority() -> Authority {
    Authority::new("intern@atlas", vec!["inventory.audit".into()])
}
