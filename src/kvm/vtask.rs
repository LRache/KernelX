use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::arch::{KvmPageFault, KvmRegs, KvmSRegs, VCpu};
use crate::fs::file::{FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{AddrSpace, MemAccessType};
use crate::kernel::syscall::UserStruct;
use crate::kernel::trap;
use crate::kernel::uapi::FileStat;
use crate::klib::SleepLock;

use super::addrspace::KvmAddrSpace;

pub enum VCpuExitReason {
    Timer,
    MemoryFault(usize, MemAccessType, usize),
    ReturnToUser(usize), // Return to user mode
}

#[repr(usize)]
#[derive(Clone, Copy, TryFromPrimitive)]
pub enum KvmInterruptKind {
    Timer = 1,
    Hardware = 2,
}

#[derive(Clone, Copy, Default)]
pub struct KvmInterruptState {
    pub timer: bool,
    pub hardware: bool,
}

impl KvmInterruptState {
    fn set_pending(&mut self, kind: KvmInterruptKind) {
        match kind {
            KvmInterruptKind::Timer => self.timer = true,
            KvmInterruptKind::Hardware => self.hardware = true,
        }
    }

    fn clear_pending(&mut self, kind: KvmInterruptKind) {
        match kind {
            KvmInterruptKind::Timer => self.timer = false,
            KvmInterruptKind::Hardware => self.hardware = false,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmInterrupt {
    pub kind: usize,
    pub irq: usize,
}

impl UserStruct for KvmInterrupt {}

enum VTaskExitReason {
    MemoryFault,
    Timer,
    Other(usize),
}

impl Into<usize> for VTaskExitReason {
    fn into(self) -> usize {
        match self {
            VTaskExitReason::MemoryFault => 1 as usize,
            VTaskExitReason::Timer => 2 as usize,
            VTaskExitReason::Other(exit_code) => exit_code as usize,
        }
    }
}

pub struct VTask {
    vcpu: VCpu,
    addrspace: Arc<KvmAddrSpace>,
    page_fault: SleepLock<Option<KvmPageFault>>,
    interrupts: SleepLock<KvmInterruptState>,
}

impl VTask {
    pub fn new(addrspace: Arc<KvmAddrSpace>) -> Self {
        Self {
            vcpu: VCpu::new(),
            addrspace,
            page_fault: SleepLock::new(None, "VTask::page_fault"),
            interrupts: SleepLock::new(KvmInterruptState::default(), "VTask::interrupts"),
        }
    }

    fn access_type_value(access_type: MemAccessType) -> usize {
        match access_type {
            MemAccessType::Read => 0,
            MemAccessType::Write => 1,
            MemAccessType::Execute => 2,
        }
    }

    fn run(&self) -> SysResult<VTaskExitReason> {
        loop {
            let interrupt_state = *self.interrupts.lock();
            match self.vcpu.run(self.addrspace.pagetable(), interrupt_state) {
                VCpuExitReason::Timer => {
                    trap::timer_interrupt();
                    if trap::handle_signal() {
                        return Err(Errno::EINTR);
                    }
                    return Ok(VTaskExitReason::Timer);
                }
                VCpuExitReason::MemoryFault(addr, access_type, inst) => {
                    if self.addrspace.try_to_fix_memory_fault(addr, access_type).is_none() {
                        *self.page_fault.lock() = Some(KvmPageFault {
                            addr,
                            access_type: Self::access_type_value(access_type),
                            inst,
                        });
                        return Ok(VTaskExitReason::MemoryFault);
                    }
                }
                VCpuExitReason::ReturnToUser(exit_code) => {
                    return Ok(VTaskExitReason::Other(exit_code));
                }
            }
        }
    }

    fn get_regs(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let regs = self.vcpu.regs();
        addrspace.copy_to_user(arg, regs)?;
        Ok(0)
    }

    fn get_sregs(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let regs: KvmSRegs = self.vcpu.sregs();
        addrspace.copy_to_user(arg, regs)?;
        Ok(0)
    }

    fn set_regs(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let regs = addrspace.copy_from_user::<KvmRegs>(arg)?;
        self.vcpu.set_regs(regs);
        Ok(0)
    }

    fn get_page_fault(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let page_fault = (*self.page_fault.lock()).ok_or(Errno::EINVAL)?;
        addrspace.copy_to_user(arg, page_fault)?;
        Ok(0)
    }

    fn interrupt_kind(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<KvmInterruptKind> {
        let interrupt = addrspace.copy_from_user::<KvmInterrupt>(arg)?;
        KvmInterruptKind::try_from(interrupt.kind).map_err(|_| Errno::EINVAL)
    }

    fn set_interrupt_pending(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let kind = self.interrupt_kind(arg, addrspace)?;
        self.interrupts.lock().set_pending(kind);
        self.vcpu.set_interrupt_pending(kind);
        Ok(0)
    }

    fn clear_interrupt_pending(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let kind = self.interrupt_kind(arg, addrspace)?;
        self.interrupts.lock().clear_pending(kind);
        self.vcpu.clear_interrupt_pending(kind);
        Ok(0)
    }
}

impl FileOps for VTask {
    fn read(&self, _buf: &mut [u8]) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn write(&self, _buf: &[u8]) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn fstat(&self) -> SysResult<FileStat> {
        Err(Errno::EOPNOTSUPP)
    }

    fn flags(&self) -> FileFlags {
        FileFlags {
            readable: false,
            writable: false,
            blocked: false,
            append: false,
            direct: false,
        }
    }

    fn ioctl(&self, request: usize, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        #[derive(TryFromPrimitive)]
        #[repr(usize)]
        enum IoctlRequest {
            Run = 1,
            GetRegs = 2,
            SetRegs = 3,
            GetSRegs = 4,
            GetPageFault = 5,
            SetInterruptPending = 6,
            ClearInterruptPending = 7,
        }

        match IoctlRequest::try_from(request) {
            Ok(IoctlRequest::Run) => self.run().map(|exit_reason| exit_reason.into()),
            Ok(IoctlRequest::GetRegs) => self.get_regs(arg, addrspace),
            Ok(IoctlRequest::SetRegs) => self.set_regs(arg, addrspace),
            Ok(IoctlRequest::GetSRegs) => self.get_sregs(arg, addrspace),
            Ok(IoctlRequest::GetPageFault) => self.get_page_fault(arg, addrspace),
            Ok(IoctlRequest::SetInterruptPending) => self.set_interrupt_pending(arg, addrspace),
            Ok(IoctlRequest::ClearInterruptPending) => self.clear_interrupt_pending(arg, addrspace),
            Err(_) => Err(Errno::EINVAL),
        }
    }
}
