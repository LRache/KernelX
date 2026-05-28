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
    MemoryFault {
        addr: usize,
        access_type: MemAccessType,
        inst: usize,
        val: usize,
    },
    ReturnToUser {
        exit_code: usize,
        inst: usize,
        val: usize,
    },
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

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmGpr {
    pub index: usize,
    pub value: usize,
}

impl UserStruct for KvmGpr {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KvmRun {
    pub inst: usize,
    pub val: usize,
}

impl UserStruct for KvmRun {}

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

    fn run(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<VTaskExitReason> {
        let run_guard = self.vcpu.enter_run()?;
        loop {
            let interrupt_state = *self.interrupts.lock();
            match self.vcpu.run(&run_guard, self.addrspace.pagetable(), interrupt_state) {
                VCpuExitReason::Timer => {
                    trap::timer_interrupt();
                    if trap::handle_signal() {
                        return Err(Errno::EINTR);
                    }
                    self.fill_run(arg, addrspace, 0, 0)?;
                    return Ok(VTaskExitReason::Timer);
                }
                VCpuExitReason::MemoryFault {
                    addr,
                    access_type,
                    inst,
                    val,
                } => {
                    if self.addrspace.try_to_fix_memory_fault(addr, access_type).is_none() {
                        *self.page_fault.lock() = Some(KvmPageFault {
                            addr,
                            access_type: Self::access_type_value(access_type),
                            inst,
                        });
                        self.fill_run(arg, addrspace, inst, val)?;
                        return Ok(VTaskExitReason::MemoryFault);
                    }
                }
                VCpuExitReason::ReturnToUser { exit_code, inst, val } => {
                    self.fill_run(arg, addrspace, inst, val)?;
                    return Ok(VTaskExitReason::Other(exit_code));
                }
            }
        }
    }

    fn fill_run(&self, arg: usize, addrspace: &AddrSpace, inst: usize, val: usize) -> SysResult<()> {
        if arg != 0 {
            addrspace.copy_to_user(arg, KvmRun { inst, val })?;
        }
        Ok(())
    }

    fn get_regs(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let regs = self.vcpu.regs();
        addrspace.copy_to_user(arg, regs)?;
        Ok(0)
    }

    fn get_sregs(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let regs: KvmSRegs = self.vcpu.sregs();
        addrspace.copy_to_user(arg, regs)?;
        Ok(0)
    }

    fn set_regs(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let regs = addrspace.copy_from_user::<KvmRegs>(arg)?;
        self.vcpu.set_regs(regs)?;
        Ok(0)
    }

    fn get_gpr(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let mut gpr = addrspace.copy_from_user::<KvmGpr>(arg)?;
        gpr.value = self.vcpu.gpr(gpr.index).ok_or(Errno::EINVAL)?;
        addrspace.copy_to_user(arg, gpr)?;
        Ok(0)
    }

    fn set_gpr(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let gpr = addrspace.copy_from_user::<KvmGpr>(arg)?;
        self.vcpu.set_gpr(gpr.index, gpr.value)?;
        Ok(0)
    }

    fn get_page_fault(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let page_fault = (*self.page_fault.lock()).ok_or(Errno::EINVAL)?;
        addrspace.copy_to_user(arg, page_fault)?;
        Ok(0)
    }

    fn interrupt_kind(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<KvmInterruptKind> {
        let interrupt = addrspace.copy_from_user::<KvmInterrupt>(arg)?;
        KvmInterruptKind::try_from(interrupt.kind).map_err(|_| Errno::EINVAL)
    }

    fn set_interrupt_pending(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let kind = self.interrupt_kind(arg, addrspace)?;
        self.interrupts.lock().set_pending(kind);
        self.vcpu.set_interrupt_pending(kind);
        Ok(0)
    }

    fn clear_interrupt_pending(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
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
            GetGpr = 8,
            SetGpr = 9,
        }

        match IoctlRequest::try_from(request).map_err(|_| Errno::EINVAL)? {
            IoctlRequest::Run => self.run(arg, addrspace).map(|exit_reason| exit_reason.into()),
            IoctlRequest::GetRegs => self.get_regs(arg, addrspace),
            IoctlRequest::SetRegs => self.set_regs(arg, addrspace),
            IoctlRequest::GetSRegs => self.get_sregs(arg, addrspace),
            IoctlRequest::GetPageFault => self.get_page_fault(arg, addrspace),
            IoctlRequest::SetInterruptPending => self.set_interrupt_pending(arg, addrspace),
            IoctlRequest::ClearInterruptPending => self.clear_interrupt_pending(arg, addrspace),
            IoctlRequest::GetGpr => self.get_gpr(arg, addrspace),
            IoctlRequest::SetGpr => self.set_gpr(arg, addrspace),
        }
    }
}
