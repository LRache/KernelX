use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{Arc, Mutex};
use std::{io, ptr};

use crate::abi::{KvmDeviceIoctl, KvmMapArea};
use crate::device::bus::{Bus, BusRef};
use crate::fd::Fd;
use crate::guest_boot::GUEST_MEMORY_BASE;
use crate::vcpu::KvmCpu;

const PAGE_SIZE: usize = 4096;

fn page_align_up(size: usize) -> Option<usize> {
    size.checked_add(PAGE_SIZE - 1).map(|size| size & !(PAGE_SIZE - 1))
}

#[derive(Clone, Copy)]
pub struct GuestMapping {
    pub guest_base: usize,
    pub guest_size: usize,
    pub host_base: *mut u8,
}

unsafe impl Send for GuestMapping {}

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
            bus: Arc::new(Mutex::new(bus)),
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.raw()
    }

    pub fn bus(&self) -> BusRef {
        self.bus.clone()
    }

    fn register_user_mapping(
        &self,
        addr: usize,
        length: usize,
        host_addr: *mut libc::c_void,
    ) -> Result<*mut u8, String> {
        let mut area = KvmMapArea {
            addr,
            length,
            mapped_addr: host_addr as usize,
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
        if area.mapped_addr != host_addr as usize {
            return Err(format!(
                "ioctl(KVM_MAP_AREA) registered unexpected host mapping: requested={host_addr:p} returned=0x{:x}",
                area.mapped_addr
            ));
        }
        Ok(area.mapped_addr as *mut u8)
    }

    pub fn map_area_raw(&self, addr: usize, length: usize) -> Result<*mut u8, String> {
        if !addr.is_multiple_of(PAGE_SIZE) {
            return Err(format!("KVM_MAP_AREA guest address is not page-aligned: 0x{addr:x}"));
        }
        let map_len = page_align_up(length)
            .ok_or_else(|| format!("KVM_MAP_AREA length overflows while page-aligning: length=0x{length:x}"))?;
        if map_len == 0 {
            return Err("KVM_MAP_AREA length is zero".to_string());
        }

        let host_addr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if host_addr == libc::MAP_FAILED {
            return Err(format!("mmap anonymous guest memory: {}", io::Error::last_os_error()));
        }

        match self.register_user_mapping(addr, length, host_addr) {
            Ok(addr) => Ok(addr),
            Err(err) => {
                unsafe {
                    libc::munmap(host_addr, map_len);
                }
                Err(err)
            }
        }
    }

    pub fn map_file_private(&self, guest_addr: usize, path: &str, label: &str) -> Result<usize, String> {
        if !guest_addr.is_multiple_of(PAGE_SIZE) {
            return Err(format!(
                "file-backed guest mapping is not page-aligned: addr=0x{guest_addr:x} path={path}"
            ));
        }

        let file = File::open(path).map_err(|err| format!("open {path}: {err}"))?;
        let size = usize::try_from(file.metadata().map_err(|err| format!("stat {path}: {err}"))?.len())
            .map_err(|_| format!("file {path} is too large to map on this host"))?;
        if size == 0 {
            return Err(format!("guest image {path} is empty"));
        }
        let map_len = page_align_up(size)
            .ok_or_else(|| format!("file {path} size overflows while page-aligning: size=0x{size:x}"))?;

        let host_addr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if host_addr == libc::MAP_FAILED {
            return Err(format!("mmap {path}: {}", io::Error::last_os_error()));
        }

        let mut bus = self.bus.lock().map_err(|_| "kvm bus lock poisoned".to_string())?;
        if let Err(err) = bus.check_host_area(guest_addr, map_len) {
            unsafe {
                libc::munmap(host_addr, map_len);
            }
            return Err(err);
        }

        let registered_addr = match self.register_user_mapping(guest_addr, size, host_addr) {
            Ok(addr) => addr,
            Err(err) => {
                unsafe {
                    libc::munmap(host_addr, map_len);
                }
                return Err(err);
            }
        };

        bus.add_host_area(guest_addr, map_len, registered_addr);

        println!("mapped {label}: 0x{size:x} bytes to guest 0x{guest_addr:x} via file {path} at {registered_addr:p}");
        Ok(size)
    }

    pub fn add_memory(&self, guest_size: usize) -> Result<GuestMapping, String> {
        let host_base = self
            .bus
            .lock()
            .map_err(|_| "kvm bus lock poisoned".to_string())?
            .map_area(self, GUEST_MEMORY_BASE, guest_size)?;
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
