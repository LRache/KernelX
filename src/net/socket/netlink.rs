//! Minimal AF_NETLINK socket implementation (NETLINK_ROUTE).
//!
//! Supports RTM_GETLINK (enumerate interfaces) and RTM_GETADDR (enumerate addresses)
//! which are the primary queries made by glibc's getifaddrs() and if_nameindex().

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use num_enum::TryFromPrimitive;

use crate::fs::file::{FileFlags, FileOps, SeekWhence};
use crate::fs::{Dentry, InodeOps, Mode};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{FileEvent, PollEventSet};
use crate::kernel::uapi::FileStat;
use crate::klib::SpinLock;
use crate::net::manager;
use crate::net::socket::AddressFamily;

// ==================== Netlink constants ====================

#[repr(usize)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
pub enum NetlinkProtocol {
    Route = 0,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
enum NlMsgType {
    Error = 2,
    Done = 3,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
enum RtMsgType {
    NewLink = 16,
    GetLink = 18,
    NewAddr = 20,
    GetAddr = 22,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
enum IfLinkAttr {
    Address = 1,
    IfName = 3,
    Mtu = 4,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
enum IfAddrAttr {
    Address = 1,
    Local = 2,
    Label = 3,
}

pub const NETLINK_ROUTE: usize = NetlinkProtocol::Route as usize;

// Netlink flags
const NLM_F_MULTI: u16 = 0x02;

// Interface flags
const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_MULTICAST: u32 = 0x1000;

const NLMSG_HDRLEN: usize = 16;
const NLA_HDRLEN: usize = 4;

// ==================== Netlink message structures ====================

/// struct nlmsghdr (16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct NlMsgHdr {
    nlmsg_len: u32,
    nlmsg_type: u16,
    nlmsg_flags: u16,
    nlmsg_seq: u32,
    nlmsg_pid: u32,
}

/// struct ifinfomsg (16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct IfInfoMsg {
    ifi_family: u8,
    _pad: u8,
    ifi_type: u16, // ARPHRD_*
    ifi_index: i32,
    ifi_flags: u32,
    ifi_change: u32,
}

/// struct ifaddrmsg (8 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct IfAddrMsg {
    ifa_family: u8,
    ifa_prefixlen: u8,
    ifa_flags: u8,
    ifa_scope: u8,
    ifa_index: u32,
}

// ==================== NetlinkSocket ====================

pub struct NetlinkSocket {
    rx_buf: SpinLock<VecDeque<u8>>,
    blocked: SpinLock<bool>,
}

impl NetlinkSocket {
    pub fn new(blocked: bool) -> Self {
        Self {
            rx_buf: SpinLock::new(VecDeque::new(), "NetlinkSocket::rx_buf"),
            blocked: SpinLock::new(blocked, "NetlinkSocket::blocked"),
        }
    }

    /// Process a netlink request and enqueue the response.
    fn process_request(&self, data: &[u8]) {
        if data.len() < NLMSG_HDRLEN {
            return;
        }

        let hdr = unsafe { &*(data.as_ptr() as *const NlMsgHdr) };
        let seq = hdr.nlmsg_seq;
        let pid = hdr.nlmsg_pid;

        match RtMsgType::try_from(hdr.nlmsg_type) {
            Ok(RtMsgType::GetLink) => self.handle_getlink(seq, pid),
            Ok(RtMsgType::GetAddr) => self.handle_getaddr(seq, pid),
            _ => self.send_error(seq, pid, -(Errno::EOPNOTSUPP as i32)),
        }
    }

    fn handle_getlink(&self, seq: u32, pid: u32) {
        let mut rx = self.rx_buf.lock();
        let ifaces = manager::list();

        for (idx, iface) in ifaces.iter().enumerate() {
            let index = (idx + 1) as i32;
            let name = iface.name();
            let mac = iface.mac_address();
            let mtu = iface.mtu() as u32;

            let mut flags: u32 = IFF_UP | IFF_RUNNING;
            if iface.is_loopback() {
                flags |= IFF_LOOPBACK;
            } else {
                flags |= IFF_MULTICAST;
            }

            let ifinfo = IfInfoMsg {
                ifi_family: 0, // AF_UNSPEC
                _pad: 0,
                ifi_type: if iface.is_loopback() { 772 } else { 1 }, // ARPHRD_LOOPBACK / ARPHRD_ETHER
                ifi_index: index,
                ifi_flags: flags,
                ifi_change: 0xFFFF_FFFF,
            };

            let mut attrs = Vec::new();
            // IFLA_IFNAME
            nla_put_string(&mut attrs, IfLinkAttr::IfName as u16, name);
            // IFLA_MTU
            nla_put_u32(&mut attrs, IfLinkAttr::Mtu as u16, mtu);
            // IFLA_ADDRESS (MAC)
            let mac_bytes = mac.as_octets();
            if !iface.is_loopback() {
                nla_put_bytes(&mut attrs, IfLinkAttr::Address as u16, mac_bytes);
            }

            let payload_len = core::mem::size_of::<IfInfoMsg>() + attrs.len();
            let msg_len = NLMSG_HDRLEN + payload_len;
            let aligned_len = nlmsg_align(msg_len);

            let hdr = NlMsgHdr {
                nlmsg_len: msg_len as u32,
                nlmsg_type: RtMsgType::NewLink as u16,
                nlmsg_flags: NLM_F_MULTI,
                nlmsg_seq: seq,
                nlmsg_pid: pid,
            };

            rx.extend(as_bytes(&hdr));
            rx.extend(as_bytes(&ifinfo));
            rx.extend(&attrs);
            // Pad to alignment
            for _ in msg_len..aligned_len {
                rx.push_back(0);
            }
        }

        // NLMSG_DONE
        self.append_done(&mut rx, seq, pid);
    }

    fn handle_getaddr(&self, seq: u32, pid: u32) {
        let mut rx = self.rx_buf.lock();
        let ifaces = manager::list();

        for (idx, iface) in ifaces.iter().enumerate() {
            let index = (idx + 1) as u32;
            let ip = match iface.ipv4() {
                Some(ip) => ip,
                None => continue,
            };
            let mask = iface.netmask();
            let prefix_len = mask.map_or(0u8, |m| u32::from(m).count_ones() as u8);

            let ifaddr = IfAddrMsg {
                ifa_family: AddressFamily::Inet as u8,
                ifa_prefixlen: prefix_len,
                ifa_flags: 0,
                ifa_scope: if iface.is_loopback() { 254 } else { 0 }, // RT_SCOPE_HOST / RT_SCOPE_UNIVERSE
                ifa_index: index,
            };

            let ip_bytes = ip.octets();
            let mut attrs = Vec::new();
            // IFA_ADDRESS
            nla_put_bytes(&mut attrs, IfAddrAttr::Address as u16, &ip_bytes);
            // IFA_LOCAL
            nla_put_bytes(&mut attrs, IfAddrAttr::Local as u16, &ip_bytes);
            // IFA_LABEL
            nla_put_string(&mut attrs, IfAddrAttr::Label as u16, iface.name());

            let payload_len = core::mem::size_of::<IfAddrMsg>() + attrs.len();
            let msg_len = NLMSG_HDRLEN + payload_len;
            let aligned_len = nlmsg_align(msg_len);

            let hdr = NlMsgHdr {
                nlmsg_len: msg_len as u32,
                nlmsg_type: RtMsgType::NewAddr as u16,
                nlmsg_flags: NLM_F_MULTI,
                nlmsg_seq: seq,
                nlmsg_pid: pid,
            };

            rx.extend(as_bytes(&hdr));
            rx.extend(as_bytes(&ifaddr));
            rx.extend(&attrs);
            for _ in msg_len..aligned_len {
                rx.push_back(0);
            }
        }

        self.append_done(&mut rx, seq, pid);
    }

    fn send_error(&self, seq: u32, pid: u32, error: i32) {
        let mut rx = self.rx_buf.lock();
        let msg_len = NLMSG_HDRLEN + 4; // nlmsghdr + error code
        let hdr = NlMsgHdr {
            nlmsg_len: msg_len as u32,
            nlmsg_type: NlMsgType::Error as u16,
            nlmsg_flags: 0,
            nlmsg_seq: seq,
            nlmsg_pid: pid,
        };
        rx.extend(as_bytes(&hdr));
        rx.extend(&error.to_ne_bytes());
    }

    fn append_done(&self, rx: &mut VecDeque<u8>, seq: u32, pid: u32) {
        let msg_len = NLMSG_HDRLEN;
        let aligned_len = nlmsg_align(msg_len);
        let hdr = NlMsgHdr {
            nlmsg_len: msg_len as u32,
            nlmsg_type: NlMsgType::Done as u16,
            nlmsg_flags: NLM_F_MULTI,
            nlmsg_seq: seq,
            nlmsg_pid: pid,
        };
        rx.extend(as_bytes(&hdr));
        for _ in msg_len..aligned_len {
            rx.push_back(0);
        }
    }
}

impl FileOps for NetlinkSocket {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize> {
        let mut rx = self.rx_buf.lock();
        if rx.is_empty() {
            return Err(Errno::EAGAIN);
        }
        let n = buf.len().min(rx.len());
        for i in 0..n {
            buf[i] = rx.pop_front().unwrap();
        }
        Ok(n)
    }

    fn pread(&self, _: &mut [u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn write(&self, buf: &[u8]) -> SysResult<usize> {
        self.process_request(buf);
        Ok(buf.len())
    }

    fn pwrite(&self, _: &[u8], _: usize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: true,
            writable: true,
            blocked: *self.blocked.lock(),
            append: false,
        }
    }

    fn seek(&self, _: isize, _: SeekWhence) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        let mut kstat = FileStat::empty();
        kstat.st_mode = Mode::S_IFSOCK.bits() as u32 | 0o666;
        kstat.st_nlink = 1;
        Ok(kstat)
    }

    fn fsync(&self) -> SysResult<()> {
        Ok(())
    }

    fn get_inode(&self) -> Option<&Arc<dyn InodeOps>> {
        None
    }

    fn get_dentry(&self) -> Option<&Arc<Dentry>> {
        None
    }

    fn wait_event(&self, _waker: usize, event: PollEventSet) -> SysResult<Option<FileEvent>> {
        if event.contains(PollEventSet::POLLIN) && !self.rx_buf.lock().is_empty() {
            return Ok(Some(FileEvent::ReadReady));
        }
        if event.contains(PollEventSet::POLLOUT) {
            return Ok(Some(FileEvent::WriteReady));
        }
        Ok(None)
    }

    fn set_flags(&self, flags: FileFlags) {
        *self.blocked.lock() = flags.blocked;
    }

    fn type_name(&self) -> &'static str {
        "netlink"
    }
}

// ==================== Helper functions ====================

/// Align to 4-byte boundary (NLMSG_ALIGN).
fn nlmsg_align(len: usize) -> usize {
    (len + 3) & !3
}

/// Append a netlink attribute with raw bytes.
fn nla_put_bytes(buf: &mut Vec<u8>, nla_type: u16, data: &[u8]) {
    let nla_len = (NLA_HDRLEN + data.len()) as u16;
    buf.extend(&nla_len.to_ne_bytes());
    buf.extend(&nla_type.to_ne_bytes());
    buf.extend(data);
    // Pad to 4-byte alignment
    let padded = nlmsg_align(nla_len as usize);
    for _ in nla_len as usize..padded {
        buf.push(0);
    }
}

/// Append a netlink attribute with a NUL-terminated string.
fn nla_put_string(buf: &mut Vec<u8>, nla_type: u16, s: &str) {
    let mut data: Vec<u8> = s.bytes().collect();
    data.push(0); // NUL terminator
    nla_put_bytes(buf, nla_type, &data);
}

/// Append a netlink attribute with a u32 value.
fn nla_put_u32(buf: &mut Vec<u8>, nla_type: u16, val: u32) {
    nla_put_bytes(buf, nla_type, &val.to_ne_bytes());
}

/// Reinterpret a struct as a byte slice.
fn as_bytes<T: Sized>(val: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts(val as *const T as *const u8, core::mem::size_of::<T>()) }
}
