//! Bluetooth SCO sockets as the kernel's `bluetooth/sco.h` defines them: the voice link between this
//! computer's adapter and a device, listened for or opened, with the voice setting of its codec.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use super::carry::Packets;

const BTPROTO_SCO: libc::c_int = 2;
const SOL_BLUETOOTH: libc::c_int = 274;
const SOL_SCO: libc::c_int = 17;
/// `SOL_SCO`: the link's packet size, once it is connected.
const SCO_OPTIONS: libc::c_int = 1;
/// `SOL_BLUETOOTH`: a listening socket hands over a link before it answers it.
const BT_DEFER_SETUP: libc::c_int = 7;
/// `SOL_BLUETOOTH`: the link's voice setting.
const BT_VOICE: libc::c_int = 11;

/// A Bluetooth device address in the kernel's byte order: the last octet of its written form first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Address([u8; 6]);

impl Address {
    /// The address written `aa:bb:cc:dd:ee:ff` or `aa-bb-cc-dd-ee-ff`.
    pub(super) fn parse(written: &str) -> Option<Self> {
        let mut octets = [0u8; 6];
        let mut parts = written.split([':', '-']);
        for octet in octets.iter_mut().rev() {
            let part = parts.next().filter(|part| part.len() == 2)?;
            *octet = u8::from_str_radix(part, 16).ok()?;
        }
        parts.next().is_none().then_some(Self(octets))
    }
}

/// What a link carries, as its voice setting says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Voice {
    /// The adapter codes narrow-band voice; the socket carries 16-bit samples.
    Coded,
    /// The adapter passes the socket's bytes as they are.
    Transparent,
}

impl Voice {
    fn setting(self) -> u16 {
        match self {
            Voice::Coded => 0x0060,
            Voice::Transparent => 0x0003,
        }
    }
}

/// `struct sockaddr_sco`.
#[repr(C)]
struct SocketAddress {
    family: libc::sa_family_t,
    address: [u8; 6],
}

impl SocketAddress {
    fn of(address: Address) -> Self {
        Self { family: libc::AF_BLUETOOTH as libc::sa_family_t, address: address.0 }
    }
}

const SOCKET_ADDRESS_LEN: libc::socklen_t = size_of::<SocketAddress>() as libc::socklen_t;

#[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
fn socket() -> io::Result<OwnedFd> {
    // SAFETY: socket(2) takes no pointers, and a descriptor it returns belongs to nothing else.
    unsafe {
        let fd = libc::socket(libc::AF_BLUETOOTH, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, BTPROTO_SCO);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

/// A socket bound to the adapter at `adapter`.
#[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
fn bound(adapter: Address) -> io::Result<OwnedFd> {
    let socket = socket()?;
    let address = SocketAddress::of(adapter);
    // SAFETY: the pointer and length describe `address`, a `sockaddr_sco`.
    let bound = unsafe { libc::bind(socket.as_raw_fd(), std::ptr::from_ref(&address).cast(), SOCKET_ADDRESS_LEN) };
    if bound != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

#[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
fn set_option<T>(socket: &impl AsRawFd, level: libc::c_int, name: libc::c_int, value: &T) -> io::Result<()> {
    // SAFETY: the pointer and length describe `value`, which lives across the call.
    let set = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            std::ptr::from_ref(value).cast(),
            size_of::<T>() as libc::socklen_t,
        )
    };
    if set == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Reads an integer-sized option into `value`.
#[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
fn read_option<T: Copy>(socket: &impl AsRawFd, level: libc::c_int, name: libc::c_int, mut value: T) -> io::Result<T> {
    let mut len = size_of::<T>() as libc::socklen_t;
    // SAFETY: the pointers describe `value` and its length, both alive across the call; the kernel
    // writes at most `len` bytes, plain integers for the options read here.
    let read =
        unsafe { libc::getsockopt(socket.as_raw_fd(), level, name, std::ptr::from_mut(&mut value).cast(), &mut len) };
    if read == 0 { Ok(value) } else { Err(io::Error::last_os_error()) }
}

/// Waits up to `timeout` for one of `events` on `socket`; the events that came, none on a timeout.
#[expect(unsafe_code, reason = "poll has no safe wrapper in the dependencies")]
fn wait(socket: &impl AsRawFd, events: libc::c_short, timeout: Duration) -> io::Result<libc::c_short> {
    let mut fds = [libc::pollfd { fd: socket.as_raw_fd(), events, revents: 0 }];
    let timeout = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    loop {
        // SAFETY: the pointer and count describe `fds`, alive across the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready >= 0 {
            return Ok(fds[0].revents);
        }

        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The error the kernel recorded for `socket`, if any.
fn pending_error(socket: &impl AsRawFd) -> io::Result<()> {
    match read_option::<libc::c_int>(socket, libc::SOL_SOCKET, libc::SO_ERROR, 0)? {
        0 => Ok(()),
        code => Err(io::Error::from_raw_os_error(code)),
    }
}

/// Waits until `socket`'s link is connected, at most `timeout`.
fn connected(socket: &impl AsRawFd, timeout: Duration) -> io::Result<()> {
    let events = wait(socket, libc::POLLOUT, timeout)?;
    if events == 0 {
        return Err(io::Error::new(ErrorKind::TimedOut, "the voice link was not connected in time"));
    }
    pending_error(socket)?;
    if events & libc::POLLOUT == 0 {
        return Err(io::Error::new(ErrorKind::ConnectionAborted, "the voice link was refused"));
    }
    Ok(())
}

/// This computer's listening socket: the links devices ask of the adapter.
pub(super) struct Listener {
    socket: OwnedFd,
}

impl Listener {
    /// Listens on the adapter at `adapter`. A link is handed over before it is answered.
    #[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
    pub(super) fn bind(adapter: Address) -> io::Result<Self> {
        let socket = bound(adapter)?;
        set_option(&socket, SOL_BLUETOOTH, BT_DEFER_SETUP, &1u32)?;
        // SAFETY: listen(2) takes no pointers.
        if unsafe { libc::listen(socket.as_raw_fd(), 1) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { socket })
    }

    /// The next link a device asks for, waited for at most `timeout`.
    #[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
    pub(super) fn next(&self, timeout: Duration) -> io::Result<Option<Asked>> {
        if wait(&self.socket, libc::POLLIN, timeout)? & libc::POLLIN == 0 {
            return Ok(None);
        }

        let mut peer = SocketAddress { family: 0, address: [0; 6] };
        let mut len = SOCKET_ADDRESS_LEN;
        // SAFETY: the pointers describe `peer` and its length, alive across the call, and a descriptor
        // accept4(2) returns belongs to nothing else.
        let socket = unsafe {
            let fd = libc::accept4(
                self.socket.as_raw_fd(),
                std::ptr::from_mut(&mut peer).cast(),
                &mut len,
                libc::SOCK_CLOEXEC,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };
        Ok(Some(Asked { socket, peer: Address(peer.address) }))
    }
}

/// A link a device asked for, not answered yet. Dropping it declines the link.
pub(super) struct Asked {
    socket: OwnedFd,
    peer: Address,
}

impl Asked {
    /// The device that asked.
    pub(super) fn peer(&self) -> Address {
        self.peer
    }

    /// Accepts the link to carry `voice`, and waits at most `timeout` for it to connect.
    pub(super) fn accept(self, voice: Voice, timeout: Duration) -> io::Result<Link> {
        set_option(&self.socket, SOL_BLUETOOTH, BT_VOICE, &voice.setting())?;
        // The first read of a link handed over unanswered answers it, and returns no bytes.
        let mut file = File::from(self.socket);
        let _no_bytes = file.read(&mut [0u8; 1])?;

        Link::connected(file, timeout)
    }
}

/// Opens a link from the adapter at `adapter` to `device` to carry `voice`, waiting at most `timeout`.
#[expect(unsafe_code, reason = "Bluetooth sockets have no safe wrapper in the dependencies")]
pub(super) fn connect(adapter: Address, device: Address, voice: Voice, timeout: Duration) -> io::Result<Link> {
    let socket = bound(adapter)?;
    set_option(&socket, SOL_BLUETOOTH, BT_VOICE, &voice.setting())?;
    let seconds = libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX);
    set_option(&socket, libc::SOL_SOCKET, libc::SO_SNDTIMEO, &libc::timeval { tv_sec: seconds, tv_usec: 0 })?;

    let address = SocketAddress::of(device);
    // SAFETY: the pointer and length describe `address`, a `sockaddr_sco`.
    let connecting =
        unsafe { libc::connect(socket.as_raw_fd(), std::ptr::from_ref(&address).cast(), SOCKET_ADDRESS_LEN) };
    if connecting != 0 {
        return Err(io::Error::last_os_error());
    }
    Link::connected(File::from(socket), timeout)
}

/// A connected voice link.
pub(super) struct Link {
    file: File,
    packet_len: usize,
}

impl Link {
    fn connected(file: File, timeout: Duration) -> io::Result<Self> {
        connected(&file, timeout)?;
        let packet_len = usize::from(read_option::<u16>(&file, SOL_SCO, SCO_OPTIONS, 0)?);
        Ok(Self { file, packet_len })
    }

    /// The length of the link's packets, as the adapter reports it.
    pub(super) fn packet_len(&self) -> usize {
        self.packet_len
    }
}

impl Packets for Link {
    fn read(&mut self, packet: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
        let events = wait(&self.file, libc::POLLIN, timeout)?;
        if events == 0 {
            return Ok(None);
        }
        if events & libc::POLLIN == 0 {
            return Ok(Some(0));
        }

        match self.file.read(packet) {
            Ok(len) => Ok(Some(len)),
            Err(e) if e.kind() == ErrorKind::ConnectionReset => Ok(Some(0)),
            Err(e) => Err(e),
        }
    }

    fn write(&mut self, packet: &[u8]) -> io::Result<()> {
        self.file.write(packet).map(|_| ())
    }
}
