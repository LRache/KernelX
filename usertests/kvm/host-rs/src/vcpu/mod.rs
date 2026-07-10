mod mmio;
mod opensbi;
mod pagetable;

use std::ffi::{c_ulong, c_void};
use std::io;
use std::os::fd::RawFd;
use std::sync::TryLockError;

use crate::abi::{
    KvmExitReason, KvmInterrupt, KvmInterruptKind, KvmPageFault, KvmReg, KvmRegs, KvmSRegs, KvmVcpuIoctl,
};
use crate::device::bus::BusRef;
use crate::fd::Fd;

pub struct KvmCpu {
    fd: Fd,
    bus: BusRef,
}

pub enum SbiCallResult {
    Resume,
    Shutdown,
    Failed,
}

impl KvmCpu {
    pub fn new(fd: Fd, bus: BusRef) -> Self {
        Self { fd, bus }
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.raw()
    }

    pub fn init(&self, pc: usize, a1: usize, a0: usize) -> Result<(), String> {
        let mut regs = KvmRegs::default();
        regs.set(KvmReg::Pc, pc);
        regs.set(KvmReg::A0, a0);
        regs.set(KvmReg::A1, a1);
        self.set_regs(&regs)
    }

    pub fn run(&self) -> Result<(), String> {
        loop {
            self.bus
                .lock()
                .map_err(|_| "kvm bus lock poisoned".to_string())?
                .update();
            self.sync_external_interrupt()?;
            let reason = self.run_once()?;
            match KvmExitReason::try_from(reason) {
                Ok(KvmExitReason::SbiCall) => {
                    let regs = self.get_regs()?;
                    match opensbi::handle_sbi_call(self, regs) {
                        SbiCallResult::Resume => continue,
                        SbiCallResult::Shutdown => return Ok(()),
                        SbiCallResult::Failed => return Err("kvm sbi call failed".to_string()),
                    }
                }
                Ok(KvmExitReason::MemoryFault) => mmio::handle_memory_fault(self)?,
                Ok(KvmExitReason::Timer) => continue,
                Err(_) => return Err(format!("unsupported kvm exit reason: 0x{reason:x}")),
            }
        }
    }

    pub fn get_regs(&self) -> Result<KvmRegs, String> {
        let mut regs = KvmRegs::default();
        let ret = unsafe {
            libc::ioctl(
                self.fd.raw(),
                KvmVcpuIoctl::GetRegs.request(),
                &mut regs as *mut KvmRegs,
            )
        };
        if ret < 0 {
            return Err(format!("ioctl(KVM_GET_REGS): {}", io::Error::last_os_error()));
        }
        Ok(regs)
    }

    pub fn get_sregs(&self) -> Result<KvmSRegs, String> {
        let mut regs = KvmSRegs::default();
        let ret = unsafe {
            libc::ioctl(
                self.fd.raw(),
                KvmVcpuIoctl::GetSregs.request(),
                &mut regs as *mut KvmSRegs,
            )
        };
        if ret < 0 {
            return Err(format!("ioctl(KVM_GET_SREGS): {}", io::Error::last_os_error()));
        }
        Ok(regs)
    }

    pub fn get_page_fault(&self) -> Result<KvmPageFault, String> {
        let mut fault = KvmPageFault::default();
        let ret = unsafe {
            libc::ioctl(
                self.fd.raw(),
                KvmVcpuIoctl::GetPageFault.request(),
                &mut fault as *mut KvmPageFault,
            )
        };
        if ret < 0 {
            return Err(format!("ioctl(KVM_GET_PAGE_FAULT): {}", io::Error::last_os_error()));
        }
        Ok(fault)
    }

    pub fn set_regs(&self, regs: &KvmRegs) -> Result<(), String> {
        let ret = unsafe {
            libc::ioctl(
                self.fd.raw(),
                KvmVcpuIoctl::SetRegs.request(),
                regs as *const KvmRegs as *mut c_void,
            )
        };
        if ret < 0 {
            return Err(format!("ioctl(KVM_SET_REGS): {}", io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn bus(&self) -> BusRef {
        self.bus.clone()
    }

    fn run_once(&self) -> Result<usize, String> {
        let ret = unsafe { libc::ioctl(self.fd.raw(), KvmVcpuIoctl::Run.request(), 0usize) };
        if ret < 0 {
            return Err(format!("ioctl(KVM_RUN): {}", io::Error::last_os_error()));
        }
        Ok(ret as usize)
    }

    fn set_interrupt_pending(&self, interrupt: KvmInterrupt) -> Result<(), String> {
        self.interrupt_ioctl(KvmVcpuIoctl::SetInterruptPending.request(), interrupt)
    }

    fn clear_interrupt_pending(&self, interrupt: KvmInterrupt) -> Result<(), String> {
        self.interrupt_ioctl(KvmVcpuIoctl::ClearInterruptPending.request(), interrupt)
    }

    fn interrupt_ioctl(&self, request: c_ulong, interrupt: KvmInterrupt) -> Result<(), String> {
        let ret = unsafe { libc::ioctl(self.fd.raw(), request, &interrupt as *const KvmInterrupt as *mut c_void) };
        if ret < 0 {
            return Err(format!("ioctl({request}): {}", io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn sync_external_interrupt(&self) -> Result<(), String> {
        let pending = self
            .bus
            .lock()
            .map_err(|_| "kvm bus lock poisoned".to_string())?
            .external_interrupt_pending();
        self.set_external_interrupt_state(pending)
    }

    pub fn try_sync_external_interrupt(&self) -> Result<(), String> {
        let pending = match self.bus.try_lock() {
            Ok(bus) => bus.external_interrupt_pending(),
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(_)) => return Err("kvm bus lock poisoned".to_string()),
        };
        self.set_external_interrupt_state(pending)
    }

    fn set_external_interrupt_state(&self, pending: bool) -> Result<(), String> {
        let interrupt = KvmInterrupt::new(KvmInterruptKind::Hardware, 0);
        if pending {
            self.set_interrupt_pending(interrupt)
        } else {
            self.clear_interrupt_pending(interrupt)
        }
    }
}
