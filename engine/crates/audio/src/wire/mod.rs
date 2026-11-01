//! The media command: an App legacy request on `/command/audio`.

use phonelink_app::{AppRequest, RequestOptions};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::TraceContext;

/// The command route.
pub const ROUTE: &str = "/command/audio";
/// `ypScenarioId` of every command.
pub const SCENARIO: &str = "SendAudioCommand";
/// `ypTriggerId` of the *Play* sent when the user picks a paused session while another one plays.
pub const TRIGGER_SOURCE_SWITCH: &str = "SelectedAudioSessionIndexChanged";

pub mod key {
    pub const CORRELATION_VECTOR: &str = "correlationVector";
    pub const AUDIO_COMMAND_VALUE: &str = "audioCommandValue";
    pub const KEY: &str = "key";
}

/// `audioCommandValue`. `0` (check permission) is never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCommand {
    Play,
    Pause,
    Next,
    Previous,
}

impl AudioCommand {
    pub fn value(self) -> i32 {
        match self {
            AudioCommand::Play => 1,
            AudioCommand::Pause => 2,
            AudioCommand::Next => 3,
            AudioCommand::Previous => 4,
        }
    }

    /// `ypTriggerId` of the button that sends it.
    pub fn button_trigger(self) -> &'static str {
        match self {
            AudioCommand::Play => "PlayButtonPressed",
            AudioCommand::Pause => "PauseButtonPressed",
            AudioCommand::Next => "NextButtonPressed",
            AudioCommand::Previous => "PreviousButtonPressed",
        }
    }
}

/// The request for `command` to the session of notification `key`.
///
/// No `contentType`, default options (not user-initiated, priority `100`), traced as user-initiated
/// with `trigger`; its `correlationVector` is the fresh trace context's correlation id.
pub fn command_request(command: AudioCommand, notification_key: &str, trigger: &str) -> AppRequest {
    let trace = TraceContext::new_root(Vec::new());
    let payload = ValueSet::new()
        .with(key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with(key::AUDIO_COMMAND_VALUE, command.value())
        .with(key::KEY, notification_key.to_string());

    AppRequest::legacy(ROUTE, payload)
        .with_options(RequestOptions::default().named(trigger, SCENARIO).traced_as_user_initiated())
        .with_trace(trace)
}

#[cfg(test)]
mod tests;
