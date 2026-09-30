//! The SDK-defined context port.
//!
//! This is the *only* thing a plugin ever receives. It exposes platform
//! services; it does not expose host internals. A plugin cannot reach the
//! registry, the plugin list, or the host because none of them are reachable
//! from here.
//!
//! Authority is carried *inside* the context, so a callee reads who is
//! calling at the moment it is called rather than trusting a thread-local or a
//! global.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::error::{SdkError, SdkResult};
use crate::identity::{Authority, ContractId, Event, Version};
use crate::value::Value;

/// A contract implementation, supplied by the plugin that declares it.
/// It receives only a context, never the provider plugin.
pub type ContractFn = Rc<dyn Fn(&Context, Value) -> SdkResult<Value>>;

/// An event subscriber.
pub type EventFn = Rc<dyn Fn(&Context, &Event) -> SdkResult<()>>;

type ContractKey = (ContractId, Version);
type EventKey = (String, u32);

/// Shared, runtime-owned state. Held behind `Rc<RefCell<_>>` because the proof
/// is single-threaded and single-host; swapping for `Arc<Mutex<_>>` is the
/// upgrade path if concurrency is ever required.
///
/// ponytail: single-threaded global lock (here, not even that — it is a plain
/// RefCell). ponytail ceiling: no concurrency. Upgrade to `Arc<Mutex<_>>` when
/// a plugin needs to run on its own thread.
#[derive(Default)]
pub struct Platform {
    contracts: RefCell<BTreeMap<ContractKey, (String, ContractFn)>>,
    subscribers: RefCell<BTreeMap<EventKey, Vec<(String, EventFn)>>>,
    audit_log: RefCell<Vec<String>>,
    logs: RefCell<Vec<String>>,
    config: RefCell<BTreeMap<String, String>>,
}

/// Runtime-only wiring. `pub(crate)` so the registry can populate the platform
/// while no plugin can reach it.
impl Platform {
    pub(crate) fn set_config(&self, key: &str, value: &str) {
        self.config
            .borrow_mut()
            .insert(key.to_string(), value.to_string());
    }

    pub(crate) fn register_contract(
        &self,
        provider: String,
        key: (ContractId, Version),
        handler: ContractFn,
    ) {
        self.contracts.borrow_mut().insert(key, (provider, handler));
    }

    pub(crate) fn register_subscription(
        &self,
        plugin: String,
        event: String,
        version: u32,
        handler: EventFn,
    ) {
        self.subscribers
            .borrow_mut()
            .entry((event, version))
            .or_default()
            .push((plugin, handler));
    }

    pub(crate) fn audit_records(&self) -> Vec<String> {
        self.audit_log.borrow().clone()
    }

    pub(crate) fn log_records(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }
}

/// The per-call context handed to plugin code.
pub struct Context {
    authority: Authority,
    platform: Rc<Platform>,
}

impl Context {
    pub fn new(authority: Authority, platform: Rc<Platform>) -> Self {
        Context {
            authority,
            platform,
        }
    }

    /// The authority of the current call. Readable by the callee itself.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn principal(&self) -> &str {
        &self.authority.principal
    }

    /// Resolve and invoke a contract through the registry. The callee receives a
    /// context carrying the *same* authority, so authority propagates instead
    /// of resetting at every hop.
    pub fn call(&self, id: &ContractId, version: Version, payload: Value) -> SdkResult<Value> {
        let found = self
            .platform
            .contracts
            .borrow()
            .get(&(id.clone(), version))
            .cloned();

        let (provider, handler) = found.ok_or_else(|| SdkError::ContractNotFound {
            contract: id.to_string(),
            version: version.to_string(),
        })?;

        self.audit(&format!(
            "call {} v{} provider={provider} principal={}",
            id, version, self.authority.principal
        ));

        let inner = Context::new(self.authority.clone(), Rc::clone(&self.platform));
        handler(&inner, payload)
    }

    /// Publish a fact. Delivery is in-process; subscribers are independent and
    /// nothing here implies a dependency between them.
    pub fn publish(&self, event: &Event) -> SdkResult<()> {
        let key = (event.id.clone(), event.version);
        let targets = self
            .platform
            .subscribers
            .borrow()
            .get(&key)
            .cloned()
            .unwrap_or_default();

        if targets.is_empty() {
            return Err(SdkError::NoSubscribers {
                event: event.id.clone(),
                version: event.version,
            });
        }

        self.audit(&format!(
            "publish {} v{} subscribers={} principal={}",
            event.id,
            event.version,
            targets.len(),
            self.authority.principal
        ));

        for (name, handler) in targets {
            let inner = Context::new(self.authority.clone(), Rc::clone(&self.platform));
            handler(&inner, event).map_err(|e| SdkError::HandlerFailed {
                plugin: name,
                reason: e.to_string(),
            })?;
        }
        Ok(())
    }

    pub fn audit(&self, message: &str) {
        self.platform
            .audit_log
            .borrow_mut()
            .push(message.to_string());
    }

    pub fn log(&self, message: &str) {
        self.platform.logs.borrow_mut().push(message.to_string());
    }

    pub fn config(&self, key: &str) -> Option<String> {
        self.platform.config.borrow().get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_with_no_subscribers_is_a_deterministic_error() {
        let platform = Rc::new(Platform::default());
        let ctx = Context::new(Authority::new("ops@atlas", vec![]), platform);
        let ev = Event::new("order.created", 1, Value::Null);
        assert_eq!(
            ctx.publish(&ev).unwrap_err(),
            SdkError::NoSubscribers {
                event: "order.created".into(),
                version: 1
            }
        );
    }

    #[test]
    fn calling_an_unregistered_contract_is_a_deterministic_error() {
        let platform = Rc::new(Platform::default());
        let ctx = Context::new(Authority::new("ops@atlas", vec![]), platform);
        let err = ctx
            .call(
                &ContractId::new("nope.missing"),
                Version::new(1, 0, 0),
                Value::Null,
            )
            .unwrap_err();
        assert_eq!(
            err,
            SdkError::ContractNotFound {
                contract: "nope.missing".into(),
                version: "1.0.0".into()
            }
        );
    }

    #[test]
    fn authority_propagates_through_a_contract_hop() {
        let platform = Rc::new(Platform::default());
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let seen_inner = Rc::clone(&seen);
        let handler: ContractFn = Rc::new(move |ctx: &Context, _p: Value| {
            seen_inner.borrow_mut().push(ctx.principal().to_string());
            Ok(Value::Null)
        });

        platform.contracts.borrow_mut().insert(
            (ContractId::new("svc.echo"), Version::new(1, 0, 0)),
            ("provider".to_string(), handler),
        );

        let caller = Context::new(
            Authority::new("ops@atlas", vec!["svc.echo".into()]),
            Rc::clone(&platform),
        );
        caller
            .call(
                &ContractId::new("svc.echo"),
                Version::new(1, 0, 0),
                Value::Null,
            )
            .unwrap();

        assert_eq!(seen.borrow().as_slice(), ["ops@atlas"]);
    }
}
