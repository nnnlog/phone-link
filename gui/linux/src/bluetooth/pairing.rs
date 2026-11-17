//! Pairing with the phone: the agent `BlueZ` asks to confirm the pairing code, which shows the code in
//! Phone Link's own question and answers with the user's choice.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use phonelink_bluetooth::PairingError;
use phonelink_core::sync::MutexExt;
use tokio::sync::oneshot;
use tracing::{info, warn};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use super::bluez::{AGENT_MANAGER, BLUEZ, Bluez};
use super::deadline::{Bounded, call_until};
use super::profile::already_registered;

const AGENT_PATH: &str = "/dev/phonelink/agent";
/// The pairing agent shows a code and asks yes or no, so `BlueZ` pairs with numeric comparison.
const AGENT_CAPABILITY: &str = "DisplayYesNo";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub(super) enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// What shows the user a pairing's question: `Some(code)` asks them to check that the phone shows
/// `code` and to confirm; `None` takes the question back.
type PairingQuestion = Box<dyn Fn(Option<String>) + Send + Sync>;

static PAIRING_QUESTION: OnceLock<PairingQuestion> = OnceLock::new();
/// Where the user's answer to the open pairing question goes.
static PAIRING_ANSWER: Mutex<Option<oneshot::Sender<bool>>> = Mutex::new(None);

/// Sets what shows a pairing's question to the user; the window wires it once, before any pairing.
pub(crate) fn on_pairing_question(show: impl Fn(Option<String>) + Send + Sync + 'static) {
    if PAIRING_QUESTION.set(Box::new(show)).is_err() {
        warn!("the pairing question was wired twice; the first wiring stays");
    }
}

/// The user's answer to the open pairing question: whether the phone shows the same code.
pub(crate) fn answer_pairing_question(confirmed: bool) {
    let answer = PAIRING_ANSWER.lock_or_recover().take();
    match answer {
        Some(answer) => {
            if answer.send(confirmed).is_err() {
                info!("a pairing answer came after its pairing ended");
            }
        }
        None => info!("a pairing answer came with no question open"),
    }
}

/// Shows the question for `passkey`; the answer comes out of the returned receiver, which fails when
/// the question is taken back. `None` when nothing can show it.
fn ask_pairing_question(passkey: u32) -> Option<oneshot::Receiver<bool>> {
    let show = PAIRING_QUESTION.get()?;
    let (sender, receiver) = oneshot::channel();
    // BlueZ asks one question at a time; an earlier one still open is over.
    drop(PAIRING_ANSWER.lock_or_recover().replace(sender));
    show(Some(format!("{passkey:06}")));
    Some(receiver)
}

/// Takes back the open pairing question, if any: its wait ends unanswered and the question closes.
fn take_back_pairing_question() {
    let open = PAIRING_ANSWER.lock_or_recover().take();
    if open.is_some()
        && let Some(show) = PAIRING_QUESTION.get()
    {
        show(None);
    }
}

fn refused<T>(kind: &'static str) -> Result<T, AgentError> {
    warn!(kind, "BlueZ asked the pairing agent for something other than a code confirmation; it is refused");
    Err(AgentError::Rejected(format!("{kind} is not supported")))
}

/// The agent of one pairing this computer starts.
///
/// `BlueZ` asks the agent of the connection that calls `Pair` ahead of the desktop's default agent, and
/// with neither pairs with Just Works without asking anyone. This agent shows the code in Phone Link
/// and answers with the user's choice. Every other request is refused.
pub(super) struct PairingAgent {
    /// The object path of the device being paired.
    pub(super) device: String,
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl PairingAgent {
    pub(super) async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32) -> Result<(), AgentError> {
        if device.as_str() != self.device {
            warn!("BlueZ asked to confirm a pairing with a device Phone Link is not pairing; it is refused");
            return Err(AgentError::Rejected("not the device being paired".to_owned()));
        }
        let Some(answer) = ask_pairing_question(passkey) else {
            warn!("no pairing question can be shown; the pairing is refused");
            return Err(AgentError::Rejected("the code cannot be shown".to_owned()));
        };

        match answer.await {
            Ok(true) => {
                info!("the user confirmed the pairing code");
                Ok(())
            }
            Ok(false) => {
                info!("the user refused the pairing");
                Err(AgentError::Rejected("the user refused the pairing".to_owned()))
            }
            Err(_) => Err(AgentError::Canceled("the pairing ended before the user answered".to_owned())),
        }
    }

    pub(super) fn cancel(&self) {
        info!("BlueZ cancelled the pairing question");
        take_back_pairing_question();
    }

    fn release(&self) {
        info!("BlueZ released the pairing agent");
    }

    fn request_pin_code(&self, _device: OwnedObjectPath) -> Result<String, AgentError> {
        refused("RequestPinCode")
    }

    fn display_pin_code(&self, _device: OwnedObjectPath, _pincode: String) -> Result<(), AgentError> {
        refused("DisplayPinCode")
    }

    pub(super) fn request_passkey(&self, _device: OwnedObjectPath) -> Result<u32, AgentError> {
        refused("RequestPasskey")
    }

    fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) -> Result<(), AgentError> {
        refused("DisplayPasskey")
    }

    fn request_authorization(&self, _device: OwnedObjectPath) -> Result<(), AgentError> {
        refused("RequestAuthorization")
    }

    pub(super) fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) -> Result<(), AgentError> {
        refused("AuthorizeService")
    }
}

/// The pairing agent, served and registered with `BlueZ` for as long as this value lives.
struct AgentRegistration<'a> {
    bus: &'a Connection,
    registered: bool,
}

impl<'a> AgentRegistration<'a> {
    fn new(bus: &'a Connection, device: &OwnedObjectPath) -> Result<Self, PairingError> {
        let failed =
            |e: zbus::Error| PairingError::Failed { reason: format!("the pairing agent was not registered: {e}") };
        if !bus.object_server().at(AGENT_PATH, PairingAgent { device: device.to_string() }).map_err(failed)? {
            return Err(PairingError::Unexpected { message: "a pairing agent is already served".to_owned() });
        }
        let mut registration = Self { bus, registered: false };

        let path = ObjectPath::try_from(AGENT_PATH).map_err(|e| failed(e.into()))?;
        match Proxy::new(bus, BLUEZ, "/org/bluez", AGENT_MANAGER)
            .and_then(|manager| manager.call::<_, _, ()>("RegisterAgent", &(path, AGENT_CAPABILITY)))
        {
            Ok(()) => {}
            // An earlier pairing's registration that was not undone names the same object.
            Err(e) if already_registered(&e) => info!("the pairing agent was still registered"),
            Err(e) => return Err(failed(e)),
        }
        registration.registered = true;
        Ok(registration)
    }
}

impl Drop for AgentRegistration<'_> {
    fn drop(&mut self) {
        if self.registered {
            let unregistered = ObjectPath::try_from(AGENT_PATH).map_err(zbus::Error::from).and_then(|path| {
                Proxy::new(self.bus, BLUEZ, "/org/bluez", AGENT_MANAGER)?.call::<_, _, ()>("UnregisterAgent", &(path,))
            });
            if let Err(e) = unregistered {
                info!(error = %e, "the pairing agent was not unregistered");
            }
        }
        if let Err(e) = self.bus.object_server().remove::<PairingAgent, _>(AGENT_PATH) {
            warn!(error = %e, "the pairing agent's object was not removed");
        }
    }
}

impl Bluez {
    /// Pairs with the device, the user confirming the code on both devices; gives up at `deadline`.
    pub(super) fn pair(&self, device: &OwnedObjectPath, deadline: Instant) -> Result<(), PairingError> {
        let agent = AgentRegistration::new(&self.bus, device)?;
        let pairing = self.device(device).map_err(|e| PairingError::Failed { reason: e.to_string() })?;

        let paired = call_until(deadline, "bluez-pair", move || pairing.call::<_, _, ()>("Pair", &()));
        let result = match paired {
            Ok(Bounded::Finished(Ok(()))) => Ok(()),
            Ok(Bounded::Finished(Err(e))) => Err(PairingError::Failed { reason: e.to_string() }),
            Ok(Bounded::DeadlinePassed) => {
                if let Err(e) = self.device(device).and_then(|proxy| proxy.call::<_, _, ()>("CancelPairing", &())) {
                    info!(error = %e, "the pairing that did not finish was not cancelled");
                }
                Err(PairingError::Failed { reason: "pairing did not finish".to_owned() })
            }
            Err(e) => Err(PairingError::Unexpected { message: e.to_string() }),
        };

        take_back_pairing_question();
        drop(agent);
        result
    }
}
