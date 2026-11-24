//! The computer's audio devices, followed on the audio server: whether a default output and a
//! microphone exist, and listeners told whenever they change.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect::Introspector;
use libpulse_binding::context::subscribe::{Facility, InterestMaskSet, Operation};
use libpulse_binding::context::{Context, FlagSet as ContextFlags, State as ContextState};
use libpulse_binding::error::PAErr;
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use phonelink_core::sync::MutexExt;

/// How long the watcher waits before it connects again after the audio server went away.
const RECONNECT_AFTER: Duration = Duration::from_secs(5);

type Listener = Arc<dyn Fn() + Send + Sync>;

/// Why the outputs stopped being followed.
#[derive(Debug, thiserror::Error)]
enum FollowError {
    #[error("no audio main loop")]
    NoMainLoop,
    #[error("no audio context")]
    NoContext,
    #[error("the audio server was not connected: {0}")]
    NotConnected(#[source] PAErr),
    #[error("the audio server refused the connection")]
    Refused,
    #[error("the audio main loop quit")]
    MainLoopQuit,
    #[error("the audio main loop failed: {0}")]
    MainLoopFailed(#[source] PAErr),
}

/// The computer's audio devices as the audio server last reported them.
#[derive(Default)]
struct Outputs {
    /// The server names a default output device, and that device exists.
    present: AtomicBool,
    /// The server names a default input device that exists and is a microphone, not the monitor of
    /// an output.
    microphone: AtomicBool,
    /// Told after every change of the default output or of the outputs there are.
    listener: Mutex<Option<Listener>>,
    /// Told after every change the first listener hears, and of the default input or the inputs.
    devices_listener: Mutex<Option<Listener>>,
}

impl Outputs {
    fn changed(&self, present: bool) {
        if self.present.swap(present, Ordering::SeqCst) != present {
            tracing::info!(present, "the computer's default audio output changed");
        }

        for listener in [&self.listener, &self.devices_listener] {
            let listener = listener.lock_or_recover().clone();
            if let Some(listener) = listener {
                listener();
            }
        }
    }

    fn microphone_changed(&self, present: bool) {
        if self.microphone.swap(present, Ordering::SeqCst) != present {
            tracing::info!(present, "the computer's microphone changed");
        }

        let listener = self.devices_listener.lock_or_recover().clone();
        if let Some(listener) = listener {
            listener();
        }
    }
}

fn outputs() -> &'static Outputs {
    static OUTPUTS: OnceLock<Outputs> = OnceLock::new();
    OUTPUTS.get_or_init(Outputs::default)
}

/// Starts following the computer's audio outputs, once.
pub fn watch_outputs() {
    static WATCHING: std::sync::Once = std::sync::Once::new();
    WATCHING.call_once(|| {
        std::thread::spawn(|| {
            loop {
                match follow_outputs(outputs()) {
                    Ok(()) => {
                        tracing::info!("the audio server went away; its outputs are followed again when it is back")
                    }
                    Err(e) => tracing::warn!(error = %e, "the audio outputs are not followed"),
                }
                outputs().changed(false);
                outputs().microphone_changed(false);
                std::thread::sleep(RECONNECT_AFTER);
            }
        });
    });
}

/// An output device exists: the audio server names a default output, and it is there.
pub fn has_output() -> bool {
    outputs().present.load(Ordering::SeqCst)
}

/// A microphone exists: the audio server names a default input that is there and is no output's
/// monitor.
pub fn has_microphone() -> bool {
    outputs().microphone.load(Ordering::SeqCst)
}

/// Calls `listener` whenever the default output or the outputs there are change.
pub fn on_outputs_changed(listener: impl Fn() + Send + Sync + 'static) {
    *outputs().listener.lock_or_recover() = Some(Arc::new(listener));
}

/// Calls `listener` whenever the default output, the default input, or the outputs or inputs there
/// are change.
pub fn on_devices_changed(listener: impl Fn() + Send + Sync + 'static) {
    *outputs().devices_listener.lock_or_recover() = Some(Arc::new(listener));
}

fn iterate(mainloop: &mut Mainloop) -> Result<(), FollowError> {
    match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        IterateResult::Quit(_) => Err(FollowError::MainLoopQuit),
        IterateResult::Err(e) => Err(FollowError::MainLoopFailed(e)),
    }
}

/// Reads whether the default output and a microphone exist, then tells `outputs`.
fn read_default_devices(introspector: &Rc<Introspector>, outputs: &'static Outputs) {
    let lookup = introspector.clone();
    introspector.get_server_info(move |server| {
        match server.default_sink_name.as_deref().filter(|name| !name.is_empty()) {
            None => outputs.changed(false),
            Some(name) => {
                let found = Cell::new(false);
                lookup.get_sink_info_by_name(name, move |result| match result {
                    ListResult::Item(_) => found.set(true),
                    ListResult::End => outputs.changed(found.get()),
                    ListResult::Error => outputs.changed(false),
                });
            }
        }

        match server.default_source_name.as_deref().filter(|name| !name.is_empty()) {
            None => outputs.microphone_changed(false),
            Some(name) => {
                let found = Cell::new(false);
                lookup.get_source_info_by_name(name, move |result| match result {
                    // With no microphone the server's default input is an output's monitor.
                    ListResult::Item(source) => found.set(source.monitor_of_sink.is_none()),
                    ListResult::End => outputs.microphone_changed(found.get()),
                    ListResult::Error => outputs.microphone_changed(false),
                });
            }
        }
    });
}

/// Connects to the audio server (waiting for it to appear), reads the default output and input and
/// reads them again whenever an output or input comes or goes or the server's defaults change; returns
/// when the server goes away.
fn follow_outputs(outputs: &'static Outputs) -> Result<(), FollowError> {
    let mut mainloop = Mainloop::new().ok_or(FollowError::NoMainLoop)?;
    let mut context = Context::new(&mainloop, "Phone Link").ok_or(FollowError::NoContext)?;
    context.connect(None, ContextFlags::NOAUTOSPAWN | ContextFlags::NOFAIL, None).map_err(FollowError::NotConnected)?;
    loop {
        iterate(&mut mainloop)?;
        match context.get_state() {
            ContextState::Ready => break,
            ContextState::Failed | ContextState::Terminated => return Err(FollowError::Refused),
            _ => {}
        }
    }

    let introspector = Rc::new(context.introspect());
    let reread = introspector.clone();
    context.set_subscribe_callback(Some(Box::new(move |facility, operation, _index| {
        let relevant = match facility {
            Some(Facility::Sink | Facility::Source) => matches!(operation, Some(Operation::New | Operation::Removed)),
            Some(Facility::Server) => true,
            _ => false,
        };
        if relevant {
            read_default_devices(&reread, outputs);
        }
    })));
    context.subscribe(InterestMaskSet::SINK | InterestMaskSet::SOURCE | InterestMaskSet::SERVER, |subscribed| {
        if !subscribed {
            tracing::warn!("the audio server's output changes are not followed; the engine keeps the first answer");
        }
    });
    read_default_devices(&introspector, outputs);

    loop {
        iterate(&mut mainloop)?;
        if matches!(context.get_state(), ContextState::Failed | ContextState::Terminated) {
            return Ok(());
        }
    }
}
