//! `inventory` — the contract provider.
//!
//! This plugin knows nothing about who consumes its contract. It receives a
//! context, does its work, and returns a value. It cannot see the plugin list,
//! the registry, or the host, because none of those are reachable from a
//! `Context`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::{Context, ContractFn};
use crate::error::{SdkError, SdkResult};
use crate::identity::{Capability, ContractDecl, Version};
use crate::manifest::{Lifecycle, Manifest};
use crate::plugin::Plugin;
use crate::value::Value;

/// The contract this plugin provides. Declared as a name plus a version; the
/// consumer declares the same name independently. They agree by identity, not
/// by sharing a symbol.
pub fn reserve_decl() -> ContractDecl {
    ContractDecl::new("inventory.reserve", Version::new(1, 0, 0))
}

pub struct Inventory {
    manifest: Manifest,
    /// Shared with the contract handler, which is a bare function.
    reserved: Rc<RefCell<u32>>,
}

impl Inventory {
    pub fn new() -> Self {
        let manifest = Manifest::new("inventory", Version::new(1, 0, 0))
            .provides(reserve_decl())
            .capability(Capability::new("reserve:stock", "inventory.reserve"))
            .lifecycle(Lifecycle::ShutdownAware);
        Inventory {
            manifest,
            reserved: Rc::new(RefCell::new(0)),
        }
    }

    pub fn reserved(&self) -> u32 {
        *self.reserved.borrow()
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Inventory {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn contracts(&self) -> Vec<(ContractDecl, ContractFn)> {
        let reserved = Rc::clone(&self.reserved);
        let handler: ContractFn = Rc::new(move |ctx: &Context, payload: Value| {
            // Authority is readable right here, at the moment of the call.
            // Not a global, not a thread-local, not assumed.
            let principal = ctx.principal().to_string();

            let quantity = payload
                .get("quantity")
                .and_then(Value::as_num)
                .ok_or_else(|| SdkError::HandlerFailed {
                    plugin: "inventory".into(),
                    reason: "payload requires a numeric 'quantity'".into(),
                })?;

            if quantity <= 0.0 {
                return Err(SdkError::HandlerFailed {
                    plugin: "inventory".into(),
                    reason: format!("quantity must be positive, got {quantity}"),
                });
            }

            let mut held = reserved.borrow_mut();
            *held += quantity as u32;

            ctx.audit(&format!("reserved {quantity} for {principal}"));
            Ok(Value::map(vec![
                ("reserved", Value::num(quantity)),
                ("on_hand", Value::num(*held as f64)),
                ("served_by", Value::str("inventory")),
                ("principal", Value::str(principal)),
            ]))
        });

        vec![(reserve_decl(), handler)]
    }

    fn on_shutdown(&mut self, ctx: &Context) -> SdkResult<()> {
        ctx.log(&format!(
            "inventory closing, {} units still reserved",
            self.reserved()
        ));
        Ok(())
    }
}
