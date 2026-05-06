use std::cell::RefCell;
use std::ffi::CString;
use std::os::fd::RawFd;
use std::rc::Rc;
use std::{io, ptr};

use crate::abi::{KvmDeviceIoctl, KvmMapArea};
use crate::device::bus::{Bus, BusRef};
use crate::fd::Fd;
use crate::guest_boot::GUEST_MEMORY_BASE;
use crate::vcpu::KvmCpu;

#[derive(Clone, Copy)]
pub struct GuestMapping {
    pub guest_base: usize,
    pub guest_size: usize,
    pub host_base: *mut u8,
}

impl GuestMapping {
    fn translate(&self, guest_addr: usize, length: usize) -> Option<*mut u8> {
        if self.host_base.is_null() || length == 0 || guest_addr < self.guest_base {
            return None;
        }
        let offset = guest_addr.checked_sub(self.guest_base)?;
        let end = offset.checked_add(length)?;
        if end > self.guest_size {
            return None;
        }
        Some(unsafe { self.host_base.add(offset) })
    }
}

pub struct Kvm {
    fd: Fd,
    bus: BusRef,
}

impl Kvm {
    pub fn open(bus: Bus) -> Result<Self, String> {
        let path = CString::new("/dev/kvm").unwrap();
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            return Err(format!("open /dev/kvm: {}", io::Error::last_os_error()));
        }
        Ok(Self {
            fd: Fd::new(fd),
            bus: Rc::new(RefCell::new(bus)),
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.raw()
    }

    pub fn bus(&self) -> BusRef {
        self.bus.clone()
    }

    pub fn map_area_raw(&self, addr: usize, length: usize) -> Result<*mut u8, String> {
        let mut area = KvmMapArea {
            addr,
            length,
            mapped_addr: 0,
        };
        let ret = unsafe {
            libc::ioctl(
                self.fd.raw(),
                KvmDeviceIoctl::MapArea.request(),
                &mut area as *mut KvmMapArea,
            )
        };
        if ret < 0 {
            return Err(format!("ioctl(KVM_MAP_AREA): {}", io::Error::last_os_error()));
        }
        if area.mapped_addr == 0 {
            return Err("ioctl(KVM_MAP_AREA) returned null host mapping".to_string());
        }
        Ok(area.mapped_addr as *mut u8)
    }

    fn map_area(&self, guest_addr: usize, length: usize) -> Result<*mut u8, String> {
        self.bus.borrow_mut().map_area(self, guest_addr, length)
    }

    pub fn add_memory(&self, guest_size: usize) -> Result<GuestMapping, String> {
        let host_base = self.map_area(GUEST_MEMORY_BASE, guest_size)?;
        Ok(GuestMapping {
            guest_base: GUEST_MEMORY_BASE,
            guest_size,
            host_base,
        })
    }

    pub fn copy_to_guest(
        &self,
        mapping: GuestMapping,
        guest_addr: usize,
        data: &[u8],
        label: &str,
    ) -> Result<(), String> {
        let Some(host_addr) = mapping.translate(guest_addr, data.len()) else {
            return Err(format!(
                "guest {label} does not fit in mapped memory: addr=0x{guest_addr:x} size=0x{:x}",
                data.len()
            ));
        };
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), host_addr, data.len());
        }
        println!(
            "loaded {label}: 0x{:x} bytes to guest 0x{guest_addr:x} via host {host_addr:p}",
            data.len()
        );
        Ok(())
    }

    pub fn create_cpu(&self) -> Result<KvmCpu, String> {
        let fd = unsafe { libc::ioctl(self.fd.raw(), KvmDeviceIoctl::CreateVcpu.request(), 0usize) };
        if fd < 0 {
            return Err(format!("ioctl(KVM_CREATE_VCPU): {}", io::Error::last_os_error()));
        }
        Ok(KvmCpu::new(Fd::new(fd as RawFd), self.bus.clone()))
    }
}
