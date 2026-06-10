use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::event::{EpollNotifier, Event, timer};
use crate::kernel::scheduler::current;
use crate::net::interface::Interface;
use crate::net::manager;

use super::{SocketAddr, SocketInner};

pub struct UdpInner {
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
    iface: Option<Arc<Interface>>,
}

impl UdpInner {
    pub fn new() -> Self {
        Self {
            local: None,
            remote: None,
            iface: None,
        }
    }

    fn route_iface(dst: SocketAddr) -> SysResult<Arc<Interface>> {
        manager::route_interface_for_dst(dst.ip).ok_or(Errno::ENETUNREACH)
    }

    fn bind_local(&mut self, addr: SocketAddr) -> SysResult<()> {
        if addr.ip.is_unspecified() {
            for iface in manager::list() {
                iface.bind_udp(addr.port);
            }
            self.iface = None;
        } else {
            let iface = manager::find_interface_for_local_addr(addr.ip).ok_or(Errno::EADDRNOTAVAIL)?;
            iface.bind_udp(addr.port);
            self.iface = Some(iface);
        }
        self.local = Some(addr);
        Ok(())
    }

    fn flow_queue(&self) -> Option<(Arc<Interface>, SocketAddr, SocketAddr)> {
        let iface = self.iface.as_ref()?.clone();
        Some((iface, self.local?, self.remote?))
    }

    fn bind_flow_queue(&self) {
        if let Some((iface, local, remote)) = self.flow_queue() {
            iface.bind_udp_flow(local.port, remote);
        }
    }

    fn unbind_flow_queue(&self) {
        if let Some((iface, local, remote)) = self.flow_queue() {
            iface.unbind_udp_flow(local.port, remote);
        }
    }

    fn try_recv_bound(&self, port: u16) -> Option<(SocketAddr, alloc::vec::Vec<u8>)> {
        if let Some((iface, local, remote)) = self.flow_queue() {
            return iface.try_recv_udp_flow(local.port, remote);
        }
        if let Some(ref iface) = self.iface {
            return iface.try_recv_udp(port);
        }
        for iface in manager::list() {
            if let Some(packet) = iface.try_recv_udp(port) {
                return Some(packet);
            }
        }
        None
    }

    fn wait_bound(&self, port: u16) -> bool {
        if let Some((iface, local, remote)) = self.flow_queue() {
            return iface.wait_udp_flow(local.port, remote);
        }
        if let Some(ref iface) = self.iface {
            return iface.wait_udp(port);
        }
        let mut ready = false;
        for iface in manager::list() {
            ready |= iface.wait_udp(port);
        }
        ready
    }

    fn cancel_wait_bound(&self, port: u16) {
        if let Some((iface, local, remote)) = self.flow_queue() {
            iface.cancel_wait_udp_flow(local.port, remote);
            return;
        }
        if let Some(ref iface) = self.iface {
            iface.cancel_wait_udp(port);
            return;
        }
        for iface in manager::list() {
            iface.cancel_wait_udp(port);
        }
    }

    fn unbind_local(&mut self, local: SocketAddr) {
        if local.ip.is_unspecified() {
            for iface in manager::list() {
                iface.unbind_udp(local.port);
            }
            self.iface = None;
        } else if let Some(iface) = self.iface.take() {
            iface.unbind_udp(local.port);
        }
    }
}

impl SocketInner for UdpInner {
    fn bind(&mut self, addr: SocketAddr) -> SysResult<()> {
        if self.local.is_some() {
            return Err(Errno::EINVAL);
        }
        if addr.port != 0 && addr.port < 1024 && current::euid() != 0 {
            return Err(Errno::EACCES);
        }

        self.bind_local(addr)
    }

    fn connect(&mut self, addr: SocketAddr, _blocked: bool) -> SysResult<()> {
        self.unbind_flow_queue();
        self.remote = Some(addr);
        // Auto-bind if not already bound
        if self.local.is_none() {
            let iface = Self::route_iface(addr)?;
            let port = iface.alloc_ephemeral_udp_port();
            iface.bind_udp(port);
            self.local = Some(SocketAddr::new(
                iface.ipv4().unwrap_or(core::net::Ipv4Addr::UNSPECIFIED),
                port,
            ));
            self.iface = Some(iface);
        } else if self.iface.is_none() {
            self.iface = Some(Self::route_iface(addr)?);
        }
        self.bind_flow_queue();
        Ok(())
    }

    fn sendto(&mut self, buf: &[u8], dst: Option<SocketAddr>, _blocked: bool) -> SysResult<usize> {
        let dst = dst.or(self.remote).ok_or(Errno::EDESTADDRREQ)?;

        // Auto-bind if not bound
        if self.local.is_none() {
            let iface = Self::route_iface(dst)?;
            let port = iface.alloc_ephemeral_udp_port();
            iface.bind_udp(port);
            self.local = Some(SocketAddr::new(
                iface.ipv4().unwrap_or(core::net::Ipv4Addr::UNSPECIFIED),
                port,
            ));
            self.iface = Some(iface);
        }

        let iface = match self.iface.as_ref() {
            Some(iface) => iface.clone(),
            None => Self::route_iface(dst)?,
        };
        let local = self.local.unwrap();

        iface.send_udp(local, dst, buf)?;
        Ok(buf.len())
    }

    fn recvfrom(
        &mut self,
        buf: &mut [u8],
        blocked: bool,
        timeout: Option<Duration>,
    ) -> SysResult<(usize, Option<SocketAddr>)> {
        let local = self.local.ok_or(Errno::EINVAL)?;
        let deadline = timeout.map(|timeout| timer::now() + timeout);

        loop {
            match self.try_recv_bound(local.port) {
                Some((src, data)) => {
                    let n = buf.len().min(data.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    return Ok((n, Some(src)));
                }
                None => {
                    if !blocked {
                        return Err(Errno::EAGAIN);
                    }
                    let timer_id = if let Some(deadline) = deadline {
                        let Some(wait_for) = deadline.checked_sub(timer::now()) else {
                            return Err(Errno::EAGAIN);
                        };
                        if wait_for.is_zero() {
                            return Err(Errno::EAGAIN);
                        }
                        Some(timer::add_timer(current::task().clone(), wait_for))
                    } else {
                        None
                    };

                    if self.wait_bound(local.port) {
                        self.cancel_wait_bound(local.port);
                        timer_id.map(timer::remove_timer);
                        continue;
                    }
                    current::schedule();
                    self.cancel_wait_bound(local.port);
                    timer_id.map(timer::remove_timer);

                    match current::task().take_wakeup_event() {
                        Some(Event::ReadReady) => continue,
                        Some(Event::Timeout) => {
                            return Err(Errno::EAGAIN);
                        }
                        Some(Event::Signal) => return Err(Errno::EINTR),
                        _ => continue,
                    }
                }
            }
        }
    }

    fn poll_read(&mut self) -> bool {
        let Some(local) = self.local else {
            return false;
        };
        if let Some((iface, local, remote)) = self.flow_queue() {
            return iface.has_udp_flow_data(local.port, remote);
        }
        if let Some(ref iface) = self.iface {
            return iface.has_udp_data(local.port);
        }
        for iface in manager::list() {
            if iface.has_udp_data(local.port) {
                return true;
            }
        }
        false
    }

    fn wait_read(&self, waker: usize) -> bool {
        let Some(local) = self.local else {
            return false;
        };
        if let Some((iface, local, remote)) = self.flow_queue() {
            return iface.wait_udp_flow_poll(local.port, remote, waker);
        }
        if let Some(ref iface) = self.iface {
            return iface.wait_udp_poll(local.port, waker);
        }

        let mut ready = false;
        for iface in manager::list() {
            ready |= iface.wait_udp_poll(local.port, waker);
        }
        ready
    }

    fn cancel_wait_read(&self) {
        if let Some(local) = self.local {
            self.cancel_wait_bound(local.port);
        }
    }

    fn epoll_notifiers(&self) -> Vec<Arc<EpollNotifier>> {
        let Some(local) = self.local else {
            return Vec::new();
        };
        if let Some((iface, local, remote)) = self.flow_queue() {
            let mut notifiers = Vec::new();
            notifiers.push(iface.udp_flow_epoll_notifier(local.port, remote));
            return notifiers;
        }
        if let Some(ref iface) = self.iface {
            let mut notifiers = Vec::new();
            notifiers.push(iface.udp_epoll_notifier(local.port));
            return notifiers;
        }
        manager::list()
            .into_iter()
            .map(|iface| iface.udp_epoll_notifier(local.port))
            .collect()
    }

    fn type_name(&self) -> &'static str {
        "inet-udp"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.remote
    }
}

impl Drop for UdpInner {
    fn drop(&mut self) {
        self.unbind_flow_queue();
        if let Some(local) = self.local.take() {
            self.unbind_local(local);
        }
    }
}
