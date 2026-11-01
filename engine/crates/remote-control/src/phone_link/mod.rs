//! Phone Link's contract: the `remoteControl` content processor. Its connect key
//! (`RemoteControlPhoneToPc = true`) is a constant the App layer's connect message writes.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use tracing::{info, warn};

use super::Inner;
use super::edge::{PowerAction, perform};
use phonelink_app::consts::result_code;
use phonelink_app::{ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, simple_answer};
use phonelink_core::codec::valueset::{Value, ValueSet};

/// The `contentType`.
pub const CONTENT_TYPE: &str = "remoteControl";
/// The command key.
pub const TYPE_KEY: &str = "remoteControlType";

/// The command names; their positions are the `remoteControlType` numbers.
pub const COMMAND_NAMES: [&str; 4] = ["LockScreen", "Sleep", "Restart", "Shutdown"];

/// What a `remoteControlType` value asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Act(PowerAction),
    /// Answered `{ result: 1 }` without acting: absent, an `Int32` outside 0…3, a string that names no
    /// command.
    Unrecognized,
    /// Refused as malformed content: a string naming a command, or a value of another type.
    Malformed(&'static str),
}

/// `0` lock, `1` sleep, `2` restart, `3` shut down — only as `Int32`.
pub fn command(payload: &ValueSet) -> Command {
    match payload.get(TYPE_KEY) {
        None => Command::Unrecognized,
        Some(Value::Int32(0)) => Command::Act(PowerAction::LockScreen),
        Some(Value::Int32(1)) => Command::Act(PowerAction::Sleep),
        Some(Value::Int32(2)) => Command::Act(PowerAction::Restart),
        Some(Value::Int32(3)) => Command::Act(PowerAction::ShutDown),
        Some(Value::Int32(_)) => Command::Unrecognized,
        Some(Value::String(s)) if COMMAND_NAMES.contains(&s.as_str()) => {
            Command::Malformed("remoteControlType names a command as text")
        }
        Some(Value::String(_)) => Command::Unrecognized,
        Some(_) => Command::Malformed("remoteControlType is not Int32"),
    }
}

pub(crate) struct RemoteControlFactory(pub(crate) Weak<Inner>);

impl ContentProcessorFactory for RemoteControlFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        Ok(self.0.upgrade().map(|inner| Box::new(RemoteControlProcessor(inner)) as Box<dyn ContentProcessor>))
    }
}

struct RemoteControlProcessor(Arc<Inner>);

#[async_trait]
impl ContentProcessor for RemoteControlProcessor {
    /// An empty answer; the processor waits for the content message.
    async fn metadata(&mut self, _ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        Processed::open(ValueSet::new())
    }

    /// The command, then `{ result: 0 }` when the Mac accepted it, else `{ result: 1 }`. The phone's
    /// toggle off answers `{ result: 1 }` without acting (differs from Phone Link on Windows, which
    /// has no switch here). A failure carries no `subError` (Phone Link on Windows sends its OS error
    /// code there).
    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let failure = || Processed::completed(simple_answer(result_code::FAILURE));
        let action = match command(payload) {
            Command::Act(action) => action,
            Command::Unrecognized => {
                warn!(phone = %ctx.phone, "remoteControl without a command it can run; answered 1");
                return failure();
            }
            Command::Malformed(reason) => return Processed::Rejected(Rejection::new(reason)),
        };

        match self.0.store.toggle(ctx.phone) {
            Ok(true) => {}
            Ok(false) => {
                info!(phone = %ctx.phone, ?action, "remoteControl refused; remote control is turned off for this phone");
                return failure();
            }
            Err(e) => {
                return Processed::Rejected(Rejection::new("remote-control setting unreadable").because(e));
            }
        }

        info!(phone = %ctx.phone, ?action, "remoteControl command received");
        if perform(self.0.power.clone(), action).await {
            Processed::completed(simple_answer(result_code::SUCCESS))
        } else {
            failure()
        }
    }
}

#[cfg(test)]
mod tests;
