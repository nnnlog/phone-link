//! RFCOMM channels to a service on the phone: opening one through `BlueZ`'s `ConnectProfile`, having
//! `BlueZ` read the phone's services again, and reading, writing and closing an open channel.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use phonelink_bluetooth::RfcommError;
use phonelink_core::sync::MutexExt;
use tracing::info;
use zbus::zvariant::OwnedObjectPath;

use super::bluez::{Bluez, POLL};
use super::deadline::{Bounded, call_until};
use super::objects::{bool_of, offers};
use super::profile::{Waiter, stop_waiting, waiter_key};

/// An open RFCOMM channel and whom it reaches.
pub(super) struct Channel {
    /// The device's object path.
    pub(super) device: String,
    /// The service on the phone (lowercase UUID).
    pub(super) service: String,
    pub(super) stream: UnixStream,
}

pub(super) type Channels = Arc<Mutex<HashMap<u64, Channel>>>;

pub(super) fn rfcomm_err(e: impl std::fmt::Display) -> RfcommError {
    RfcommError::Unexpected { message: format!("BlueZ: {e}") }
}

/// `BlueZ`'s failure texts for a device that did not answer the page (out of range, Bluetooth off on it).
fn says_unreachable(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("page-timeout") || lower.contains("host-down") || lower.contains("host is down")
}

fn connect_err(e: zbus::Error) -> RfcommError {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    if says_unreachable(&text) {
        RfcommError::Unreachable
    } else if lower.contains("notavailable") || lower.contains("profile-unavailable") || lower.contains("not found") {
        RfcommError::ServiceNotFound
    } else {
        RfcommError::ConnectFailed { reason: text }
    }
}

impl Bluez {
    /// Polls the device's services until they include `service` or `deadline` passes; whether they do.
    fn wait_for_service(&self, path: &OwnedObjectPath, service: &str, deadline: Instant) -> Result<bool, RfcommError> {
        loop {
            let offered =
                self.device_properties(path).map_err(rfcomm_err)?.is_some_and(|device| offers(&device, service));
            let left = deadline.saturating_duration_since(Instant::now());
            if offered || left.is_zero() {
                return Ok(offered);
            }
            thread::sleep(POLL.min(left));
        }
    }

    /// Opens an RFCOMM channel to `service_uuid` on the device at `address`; its number.
    pub(super) fn rfcomm_open(&self, address: &str, service_uuid: &str, timeout_ms: u64) -> Result<u64, RfcommError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let uuid = service_uuid.to_ascii_lowercase();
        self.ensure_profile(&uuid)?;
        let path = self.device_path(address).map_err(rfcomm_err)?.ok_or(RfcommError::Unreachable)?;
        let device = self.device(&path).map_err(rfcomm_err)?;

        let key = waiter_key(path.as_str(), &uuid);
        let id = self.next_waiter.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.waiters.lock_or_recover().insert(key.clone(), Waiter { id, sender: sender.clone() });

        // BlueZ answers ConnectProfile once the profile's handler took the connection, which went to
        // the waiter, or with why it did not connect; either ends the wait, and so does the deadline.
        let profile = uuid.clone();
        let connecting = thread::Builder::new().name("bluez-connect".to_owned()).spawn(move || {
            if let Err(e) = device.call::<_, _, ()>("ConnectProfile", &(profile.as_str(),)) {
                // The open gave up when nobody receives.
                let _ = sender.send(Err(connect_err(e)));
            }
        });
        let outcome = match connecting {
            Ok(_) => receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            Err(e) => Ok(Err(RfcommError::Unexpected { message: format!("the connect thread did not start: {e}") })),
        };
        stop_waiting(&self.waiters, &key, id, &receiver);
        let fd = match outcome {
            Ok(opened) => opened?,
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(RfcommError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(RfcommError::Unexpected { message: "the open's waiter went away".to_owned() });
            }
        };

        let stream = UnixStream::from(std::os::fd::OwnedFd::from(fd));
        stream.set_nonblocking(false).map_err(|e| RfcommError::Io { reason: e.to_string() })?;
        let channel = self.next_channel.fetch_add(1, Ordering::SeqCst);
        self.channels.lock_or_recover().insert(channel, Channel { device: path.to_string(), service: uuid, stream });
        Ok(channel)
    }

    /// Has `BlueZ` read the device's services again when they lack `service_uuid`; whether they have it.
    ///
    /// A `ConnectProfile` for a service `BlueZ` does not know on the device makes it browse the device's
    /// service records, and connects nothing else (a connection to the service, if it is there, goes to
    /// no open and is shut down). Everything ends by the deadline, and the device is disconnected again
    /// when it was not connected before.
    pub(super) fn refresh_services(
        &self,
        address: &str,
        service_uuid: &str,
        timeout_ms: u64,
    ) -> Result<bool, RfcommError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let uuid = service_uuid.to_ascii_lowercase();
        let path = self.device_path(address).map_err(rfcomm_err)?.ok_or(RfcommError::Unreachable)?;
        let device = self.device_properties(&path).map_err(rfcomm_err)?.ok_or(RfcommError::Unreachable)?;
        if offers(&device, &uuid) {
            return Ok(true);
        }
        let connected_before = bool_of(&device, "Connected");

        self.ensure_profile(&uuid)?;
        let proxy = self.device(&path).map_err(rfcomm_err)?;
        let profile = uuid.clone();
        let browsed = call_until(deadline, "bluez-browse", move || {
            proxy.call::<_, _, ()>("ConnectProfile", &(profile.as_str(),))
        });
        let result = match browsed {
            Ok(Bounded::Finished(Err(e))) if says_unreachable(&e.to_string()) => Err(RfcommError::Unreachable),
            Ok(outcome) => {
                if let Bounded::Finished(Err(e)) = outcome {
                    info!(error = %e, service_uuid = %uuid, "BlueZ did not connect the service it browsed for");
                }
                self.wait_for_service(&path, &uuid, deadline)
            }
            Err(e) => Err(RfcommError::Unexpected { message: e.to_string() }),
        };

        if !connected_before {
            // Also ends a browse or connection BlueZ has not finished.
            if let Err(e) = self.device(&path).and_then(|proxy| proxy.call::<_, _, ()>("Disconnect", &())) {
                info!(error = %e, "the connection that read the phone's services was not closed");
            }
        }
        result
    }

    fn with_channel<T>(
        &self,
        channel: u64,
        f: impl FnOnce(&mut UnixStream) -> Result<T, RfcommError>,
    ) -> Result<T, RfcommError> {
        let channels = self.channels.lock_or_recover();
        let open = channels.get(&channel).ok_or(RfcommError::Io { reason: "the channel is closed".to_owned() })?;
        let mut stream = open.stream.try_clone().map_err(|e| RfcommError::Io { reason: e.to_string() })?;
        drop(channels);
        f(&mut stream)
    }

    pub(super) fn rfcomm_write(&self, channel: u64, bytes: &[u8]) -> Result<(), RfcommError> {
        self.with_channel(channel, |stream| {
            stream.write_all(bytes).map_err(|e| RfcommError::Io { reason: e.to_string() })
        })
    }

    /// Reads what the channel has, up to 4096 bytes, waiting up to `timeout_ms` for it.
    pub(super) fn rfcomm_read(&self, channel: u64, timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
        self.with_channel(channel, |stream| {
            stream
                .set_read_timeout(Some(Duration::from_millis(timeout_ms.max(1))))
                .map_err(|e| RfcommError::Io { reason: e.to_string() })?;
            let mut buffer = vec![0; 4096];
            match stream.read(&mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(buffer)
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Err(RfcommError::TimedOut),
                Err(e) => Err(RfcommError::Io { reason: e.to_string() }),
            }
        })
    }

    pub(super) fn rfcomm_close(&self, channel: u64) {
        if let Some(open) = self.channels.lock_or_recover().remove(&channel) {
            let _ = open.stream.shutdown(Shutdown::Both);
        }
    }
}
