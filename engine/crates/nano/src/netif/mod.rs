//! The host addresses ICE gathers on: every up interface's IPv4 and IPv6 addresses, without
//! loopback, site-local, IPv4-mapped/compatible and unspecified ones (link-local IPv6 stays).

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use super::ice::candidate::{AddressType, classify};

/// One host address and its interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostAddress {
    /// With the interface's scope id for a link-local IPv6 address.
    pub address: SocketAddr,
    pub interface_index: u32,
}

fn usable(ip: IpAddr) -> bool {
    !matches!(
        classify(ip),
        AddressType::Loopback
            | AddressType::SiteLocal
            | AddressType::Ipv4Mapped
            | AddressType::Ipv4Compatible
            | AddressType::Unspecified
            | AddressType::Multicast
            | AddressType::Invalid
    )
}

/// The usable addresses of the up interfaces, port 0.
#[expect(unsafe_code, reason = "getifaddrs has no safe wrapper in the dependencies")]
pub fn host_addresses() -> std::io::Result<Vec<HostAddress>> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();

    // SAFETY: getifaddrs fills `list` with a linked list that freeifaddrs releases below; every
    // pointer read is checked for null and stays within that list's lifetime.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut node = list;
        while !node.is_null() {
            let ifa = &*node;
            node = ifa.ifa_next;
            let up = ifa.ifa_flags & (libc::IFF_UP as u32) != 0 && ifa.ifa_flags & (libc::IFF_RUNNING as u32) != 0;
            if !up || ifa.ifa_addr.is_null() {
                continue;
            }

            let index = libc::if_nametoindex(ifa.ifa_name);
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy();
            let family = i32::from((*ifa.ifa_addr).sa_family);
            let address = if family == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                SocketAddr::new(IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))), 0)
            } else if family == libc::AF_INET6 {
                let sin6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                let scope = if classify(IpAddr::V6(ip)) == AddressType::LinkLocal { index } else { 0 };
                SocketAddr::V6(SocketAddrV6::new(ip, 0, 0, scope))
            } else {
                continue;
            };

            if usable(address.ip()) {
                tracing::trace!(interface = %name, %address, "host address");
                out.push(HostAddress { address, interface_index: index });
            }
        }

        libc::freeifaddrs(list);
    }

    Ok(out)
}

#[cfg(test)]
mod tests;
