//! The Rust binding for a foreign-language plugin.
//!
//! A [`ForeignPlugin`] looks and behaves exactly like a native plugin: it
//! implements the same [`Plugin`] trait, so the registry, the context port and
//! the authority checks cannot tell the difference. The only difference is that
//! handlers cross a process boundary as JSON.
//!
//! This is the evidence that the boundary is a protocol and not a Rust object
//! model: nothing in `runtime.rs` or `context.rs` changed to accommodate a
//! plugin written in another language.
//!
//! ponytail: one process, one synchronous request/response at a time, no
//! pipelining and no nested calls. A foreign plugin can be invoked and can
//! consume events, but it cannot itself invoke another contract mid-call —
//! that needs a correlated reply protocol. Upgrade when a real plugin needs it.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::context::{Context, ContractFn};
use crate::error::{SdkError, SdkResult};
use crate::identity::{ContractDecl, Event};
use crate::manifest::Manifest;
use crate::plugin::{subscription, Plugin, Subscription};
use crate::protocol;
use crate::value::Value;

/// How long the runtime waits for a plugin to answer before giving up.
///
/// ponytail: a fixed ceiling, not adaptive. A plugin that cannot answer within
/// five seconds is broken, and a broken plugin must not be able to wedge the
/// host — which is exactly what an unbounded `read_line` allowed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The process boundary. Shared by every handler the bridge registers, because
/// the trait hands out `&Context` handlers while the pipe needs `&mut`.
///
/// Reading happens on a dedicated thread feeding a channel, so the wait is
/// bounded by a timeout rather than by however long the plugin feels like
/// taking. Blocking reads cannot be interrupted portably with std alone.
struct Transport {
    command: String,
    stdin: Option<ChildStdin>,
    rx: Option<Receiver<String>>,
    timeout: Duration,
}

impl Transport {
    fn send(&mut self, message: &Value) -> SdkResult<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| SdkError::ProtocolViolation {
                detail: format!("plugin '{}' stdin is closed", self.command),
            })?;
        writeln!(stdin, "{}", crate::json::to_json(message))
            .and_then(|_| stdin.flush())
            .map_err(|e| SdkError::ProtocolViolation {
                detail: format!("failed to write to plugin '{}': {e}", self.command),
            })
    }

    fn recv(&mut self) -> SdkResult<Value> {
        let rx = self
            .rx
            .as_ref()
            .ok_or_else(|| SdkError::ProtocolViolation {
                detail: format!("plugin '{}' stdout is closed", self.command),
            })?;

        let line = match rx.recv_timeout(self.timeout) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                return Err(SdkError::PluginTimeout {
                    command: self.command.clone(),
                    millis: self.timeout.as_millis() as u64,
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(SdkError::ProtocolViolation {
                    detail: format!("plugin '{}' closed its output unexpectedly", self.command),
                })
            }
        };

        crate::json::parse_json(line.trim()).map_err(|e| SdkError::ProtocolViolation {
            detail: format!("plugin '{}' emitted invalid JSON: {e}", self.command),
        })
    }

    fn exchange(&mut self, message: &Value) -> SdkResult<Value> {
        self.send(message)?;
        self.recv()
    }
}

/// Move the child's stdout onto a thread that pumps lines into a channel.
fn spawn_reader(stdout: ChildStdout) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                // EOF: drop `tx`, which disconnects the receiver and lets a
                // waiting caller fail fast instead of hanging.
                Ok(0) => return,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    rx
}

/// A plugin living in another process, spoken to over stdin/stdout.
pub struct ForeignPlugin {
    manifest: Manifest,
    transport: Rc<RefCell<Transport>>,
    child: Option<Child>,
}

impl ForeignPlugin {
    /// Spawn the plugin and complete the handshake, returning a ready instance.
    ///
    /// The protocol version is checked here, so a plugin speaking an
    /// unsupported version is refused before it can register or start.
    pub fn connect(command: impl Into<String>, args: Vec<String>) -> SdkResult<Self> {
        Self::connect_with_timeout(command, args, DEFAULT_TIMEOUT)
    }

    /// As `connect`, with an explicit ceiling on how long the plugin may take
    /// to answer any single message.
    pub fn connect_with_timeout(
        command: impl Into<String>,
        args: Vec<String>,
        timeout: Duration,
    ) -> SdkResult<Self> {
        let command = command.into();

        let mut child = Command::new(&command)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Plugins must not write into the host's console.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| SdkError::ForeignPluginSpawn {
                command: command.clone(),
                reason: e.to_string(),
            })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let rx = spawn_reader(child.stdout.take().expect("stdout was piped"));

        let transport = Rc::new(RefCell::new(Transport {
            command: command.clone(),
            stdin: Some(stdin),
            rx: Some(rx),
            timeout,
        }));

        let manifest = {
            let mut t = transport.borrow_mut();
            t.exchange(&protocol::hello())?
        };
        let manifest = protocol::read_manifest(&manifest)?;

        Ok(ForeignPlugin {
            manifest,
            transport,
            child: Some(child),
        })
    }

    /// Send shutdown, then reap the process.
    pub fn shutdown(&mut self) -> SdkResult<()> {
        if self.child.is_none() {
            return Ok(());
        }
        {
            let mut t = self.transport.borrow_mut();
            let _ = t.exchange(&protocol::ack("shutdown"));
            t.stdin = None;
            t.rx = None;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    /// Whether the plugin has been shut down and reaped.
    pub fn stopped(&self) -> bool {
        self.child.is_none()
    }

    /// Invoke a declared contract directly over the wire, presenting
    /// `principal` as the caller. This is the entry point a conformance suite
    /// uses; the runtime reaches the same pipe through `contracts()`.
    pub fn invoke_contract(
        &mut self,
        contract: &crate::identity::ContractId,
        version: crate::identity::Version,
        principal: &str,
        payload: Value,
    ) -> SdkResult<Value> {
        let reply = self
            .transport
            .borrow_mut()
            .exchange(&protocol::invoke(contract, version, principal, &payload))?;

        match protocol::read_result(&reply)? {
            Ok(value) => Ok(value),
            Err(e) => Err(SdkError::HandlerFailed {
                plugin: self.manifest.id.clone(),
                reason: e.to_string(),
            }),
        }
    }

    /// Deliver an event directly over the wire, presenting `principal` as the
    /// publisher.
    pub fn deliver_event(&mut self, event: &Event, principal: &str) -> SdkResult<()> {
        self.transport
            .borrow_mut()
            .exchange(&protocol::event(event, principal))?;
        Ok(())
    }
}

impl Plugin for ForeignPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// One handler per declared contract, each forwarding over the pipe.
    fn contracts(&self) -> Vec<(ContractDecl, ContractFn)> {
        let transport = Rc::clone(&self.transport);
        let provider = self.manifest.id.clone();

        self.manifest
            .contracts_provided
            .iter()
            .map(|decl| {
                let transport = Rc::clone(&transport);
                let provider = provider.clone();
                let contract = decl.id.clone();
                let version = decl.version;

                let handler: ContractFn = Rc::new(move |ctx: &Context, payload: Value| {
                    let message = protocol::invoke(&contract, version, ctx.principal(), &payload);
                    let reply = transport.borrow_mut().exchange(&message)?;
                    match protocol::read_result(&reply)? {
                        Ok(value) => {
                            ctx.audit(&format!(
                                "foreign call {contract} v{version} provider={provider} principal={}",
                                ctx.principal()
                            ));
                            Ok(value)
                        }
                        Err(e) => Err(SdkError::HandlerFailed {
                            plugin: provider.clone(),
                            reason: e.to_string(),
                        }),
                    }
                });

                (decl.clone(), handler)
            })
            .collect()
    }

    /// One handler per declared event, each forwarding over the pipe.
    fn subscriptions(&self) -> Vec<Subscription> {
        self.manifest
            .subscriptions
            .iter()
            .map(|event| {
                let transport = Rc::clone(&self.transport);
                subscription(
                    event.id.clone(),
                    event.version,
                    move |ctx: &Context, e: &crate::identity::Event| {
                        let message = protocol::event(e, ctx.principal());
                        transport.borrow_mut().exchange(&message)?;
                        Ok(())
                    },
                )
            })
            .collect()
    }

    fn on_initialize(&mut self, ctx: &Context) -> SdkResult<()> {
        ctx.log(&format!(
            "{} initialised over the protocol (v{})",
            self.manifest.id,
            protocol::PROTOCOL_VERSION
        ));
        Ok(())
    }

    fn on_shutdown(&mut self, ctx: &Context) -> SdkResult<()> {
        ctx.log(&format!(
            "{} shutting down over the protocol",
            self.manifest.id
        ));
        self.shutdown()
    }
}

/// A dropped bridge must not leave an orphaned process behind.
impl Drop for ForeignPlugin {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Encode a message for the wire, so a binding in another language — or a
/// test — can speak the protocol without going through `ForeignPlugin`.
pub fn encode(message: &Value) -> String {
    format!("{}\n", crate::json::to_json(message))
}

/// Decode a line from the wire.
pub fn decode(line: &str) -> SdkResult<Value> {
    crate::json::parse_json(line.trim()).map_err(|e| SdkError::ProtocolViolation {
        detail: format!("invalid JSON on the wire: {e}"),
    })
}
