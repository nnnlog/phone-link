//! Generic netlink as the kernel's `linux/netlink.h` and `linux/genetlink.h` define it: a socket that
//! asks the kernel one question at a time, and the messages and attributes of its requests and answers.

use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[cfg(test)]
mod tests;

// Headers and attribute values are in the host's byte order.
const NLMSG_HDRLEN: usize = 16;
pub(super) const GENL_HDRLEN: usize = 4;
const NLA_HDRLEN: usize = 4;
const NLMSG_NOOP: u16 = 1;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_DUMP: u16 = 0x300;
/// An attribute's type without its `NLA_F_NESTED` and `NLA_F_NET_BYTEORDER` flags.
const NLA_TYPE_MASK: u16 = 0x3fff;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
/// How long the kernel may take to answer before the read fails.
const NETLINK_ANSWER_TIMEOUT_SECS: libc::time_t = 5;
/// Room for one netlink datagram; one that does not fit is an error rather than cut short.
const NETLINK_DATAGRAM_LEN: usize = 64 * 1024;

/// A generic netlink socket to the kernel, for one question at a time.
pub(super) struct GenericNetlink {
    socket: OwnedFd,
    /// The sequence number of the last request.
    sequence: u32,
}

impl GenericNetlink {
    #[expect(unsafe_code, reason = "netlink sockets have no safe wrapper in the dependencies")]
    pub(super) fn open() -> io::Result<Self> {
        // SAFETY: socket(2) takes no pointers, and a descriptor it returns belongs to nothing else.
        let socket = unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_GENERIC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedFd::from_raw_fd(fd)
        };
        let timeout = libc::timeval { tv_sec: NETLINK_ANSWER_TIMEOUT_SECS, tv_usec: 0 };
        // SAFETY: the pointer and length describe `timeout`, the `timeval` SO_RCVTIMEO takes.
        let set = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                std::ptr::from_ref(&timeout).cast(),
                size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if set != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { socket, sequence: 0 })
    }

    /// The id of the generic netlink family `name`.
    pub(super) fn family_id(&mut self, name: &str) -> io::Result<u16> {
        let mut attributes = Vec::new();
        put_attribute(&mut attributes, CTRL_ATTR_FAMILY_NAME, format!("{name}\0").as_bytes());
        let answers = self.ask(GENL_ID_CTRL, false, CTRL_CMD_GETFAMILY, 1, &attributes)?;
        let answer = answers.first().ok_or_else(|| malformed("the generic netlink controller gave no answer"))?;
        family_id_of(answer)
    }

    /// Asks `family` the `command` with `attributes`, and returns the payloads of its answers (each a
    /// generic netlink header and attributes): the one answer to a request, or every one of a dump.
    pub(super) fn ask(
        &mut self,
        family: u16,
        dump: bool,
        command: u8,
        version: u8,
        attributes: &[u8],
    ) -> io::Result<Vec<Vec<u8>>> {
        self.sequence += 1;
        let flags = if dump { NLM_F_REQUEST | NLM_F_DUMP } else { NLM_F_REQUEST };
        self.send(&generic_message(family, flags, self.sequence, command, version, attributes))?;
        let mut answers = Vec::new();
        let mut buffer = vec![0; NETLINK_DATAGRAM_LEN];
        loop {
            let received = self.receive(&mut buffer)?;
            if take_answers(&buffer[..received], family, self.sequence, dump, &mut answers)? {
                return Ok(answers);
            }
        }
    }

    #[expect(unsafe_code, reason = "netlink sockets have no safe wrapper in the dependencies")]
    fn send(&self, message: &[u8]) -> io::Result<()> {
        // An unconnected netlink socket sends a message with no destination address to the kernel.
        // SAFETY: the pointer and length describe `message`.
        let sent = unsafe { libc::send(self.socket.as_raw_fd(), message.as_ptr().cast(), message.len(), 0) };

        match usize::try_from(sent) {
            Ok(sent) if sent == message.len() => Ok(()),
            Ok(_) => Err(io::Error::other("a netlink request was sent in part")),
            Err(_) => Err(io::Error::last_os_error()),
        }
    }

    /// Receives one datagram into `buffer` and returns its length.
    #[expect(unsafe_code, reason = "netlink sockets have no safe wrapper in the dependencies")]
    fn receive(&self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            // With MSG_TRUNC, recv(2) returns a datagram's whole length even when it did not fit.
            // SAFETY: the pointer and length describe `buffer`.
            let received = unsafe {
                libc::recv(self.socket.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), libc::MSG_TRUNC)
            };
            let Ok(received) = usize::try_from(received) else {
                let error = io::Error::last_os_error();
                match error.kind() {
                    ErrorKind::Interrupted => continue,
                    ErrorKind::WouldBlock => {
                        return Err(io::Error::new(ErrorKind::TimedOut, "the kernel did not answer"));
                    }
                    _ => return Err(error),
                }
            };
            if received > buffer.len() {
                return Err(io::Error::other(format!("a netlink answer of {received} bytes did not fit")));
            }
            return Ok(received);
        }
    }
}

/// Adds the answers in `datagram` to those of request `sequence` to `family`; `true` once the answers
/// are complete: the one to a request, or every one of a dump once the dump is done. A failure the
/// kernel reports is the error with its number.
fn take_answers(
    datagram: &[u8],
    family: u16,
    sequence: u32,
    dump: bool,
    answers: &mut Vec<Vec<u8>>,
) -> io::Result<bool> {
    for message in netlink_messages(datagram)? {
        if message.sequence != sequence {
            continue;
        }
        match message.kind {
            // An error message starts with the error number (0 acknowledges), the end of a dump with
            // the dump's.
            NLMSG_ERROR | NLMSG_DONE => {
                let code = i32_at(message.payload, 0).ok_or_else(|| malformed("a netlink status is cut short"))?;
                if code < 0 {
                    return Err(io::Error::from_raw_os_error(code.saturating_neg()));
                }
                return Ok(true);
            }
            NLMSG_NOOP => {}
            kind if kind == family => {
                answers.push(message.payload.to_vec());
                if !dump {
                    return Ok(true);
                }
            }
            kind => return Err(malformed(&format!("a netlink message of type {kind}"))),
        }
    }

    Ok(false)
}

/// One message of a netlink datagram.
#[derive(Debug, PartialEq, Eq)]
struct NetlinkMessage<'a> {
    kind: u16,
    sequence: u32,
    payload: &'a [u8],
}

/// The messages of `datagram`, in order.
fn netlink_messages(mut datagram: &[u8]) -> io::Result<Vec<NetlinkMessage<'_>>> {
    let mut messages = Vec::new();
    while !datagram.is_empty() {
        let (Some(len), Some(kind), Some(sequence)) = (u32_at(datagram, 0), u16_at(datagram, 4), u32_at(datagram, 8))
        else {
            return Err(malformed("a netlink message header is cut short"));
        };
        let len = len as usize;
        if len < NLMSG_HDRLEN || len > datagram.len() {
            return Err(malformed(&format!("a netlink message claims {len} bytes")));
        }
        messages.push(NetlinkMessage { kind, sequence, payload: &datagram[NLMSG_HDRLEN..len] });
        datagram = &datagram[aligned(len).min(datagram.len())..];
    }

    Ok(messages)
}

/// The attributes in `bytes`, each as its type (without flags) and its payload.
pub(super) fn netlink_attributes(mut bytes: &[u8]) -> io::Result<Vec<(u16, &[u8])>> {
    let mut attributes = Vec::new();
    while !bytes.is_empty() {
        let (Some(len), Some(kind)) = (u16_at(bytes, 0), u16_at(bytes, 2)) else {
            return Err(malformed("a netlink attribute header is cut short"));
        };
        let len = usize::from(len);
        if len < NLA_HDRLEN || len > bytes.len() {
            return Err(malformed(&format!("a netlink attribute claims {len} bytes")));
        }
        attributes.push((kind & NLA_TYPE_MASK, &bytes[NLA_HDRLEN..len]));
        bytes = &bytes[aligned(len).min(bytes.len())..];
    }

    Ok(attributes)
}

/// The payload of the attribute `kind` among `attributes`.
pub(super) fn attribute_of<'a>(attributes: &[(u16, &'a [u8])], kind: u16) -> Option<&'a [u8]> {
    attributes.iter().find(|(found, _)| *found == kind).map(|(_, payload)| *payload)
}

/// A generic netlink message: the netlink header (its sender's port left 0, as the kernel knows the
/// sender by its socket), the generic netlink header, then the attributes.
fn generic_message(family: u16, flags: u16, sequence: u32, command: u8, version: u8, attributes: &[u8]) -> Vec<u8> {
    let len = NLMSG_HDRLEN + GENL_HDRLEN + attributes.len();
    let mut message = Vec::with_capacity(len);
    message.extend_from_slice(&u32::try_from(len).expect("a request is far below 4 GiB").to_ne_bytes());
    message.extend_from_slice(&family.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    message.extend_from_slice(&sequence.to_ne_bytes());
    message.extend_from_slice(&0u32.to_ne_bytes());
    message.extend_from_slice(&[command, version, 0, 0]);
    message.extend_from_slice(attributes);
    message
}

/// Appends the attribute `kind` holding `payload` to `attributes`, padded to four bytes.
pub(super) fn put_attribute(attributes: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    let len = u16::try_from(NLA_HDRLEN + payload.len()).expect("an attribute is far below 64 KiB");
    attributes.extend_from_slice(&len.to_ne_bytes());
    attributes.extend_from_slice(&kind.to_ne_bytes());
    attributes.extend_from_slice(payload);
    attributes.resize(aligned(attributes.len()), 0);
}

/// `len` rounded up to the four-byte alignment of netlink messages and attributes.
fn aligned(len: usize) -> usize {
    len.next_multiple_of(4)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

pub(super) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_ne_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// An answer that does not read as netlink, with what is wrong with it.
pub(super) fn malformed(what: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, format!("malformed netlink answer: {what}"))
}

/// The family id in the generic netlink controller's answer to `CTRL_CMD_GETFAMILY`.
fn family_id_of(answer: &[u8]) -> io::Result<u16> {
    let attributes = netlink_attributes(answer.get(GENL_HDRLEN..).unwrap_or_default())?;
    attribute_of(&attributes, CTRL_ATTR_FAMILY_ID)
        .and_then(|id| u16_at(id, 0))
        .ok_or_else(|| malformed("the controller's answer names no family id"))
}
