use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::arch::{KvmRegs, VCpu};
use crate::fs::file::{FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{AddrSpace, MemAccessType};
use crate::kernel::trap;
use crate::kernel::uapi::FileStat;
use crate::klib::SleepLock;

pub enum VCpuExitReason {
    Timer,
    MemoryFault(usize, MemAccessType),
    ReturnToUser(usize), // Return to user mode
}

enum VTaskExitReason {
    MemoryFault(usize, MemAccessType),
    Other(usize),
}

impl Into<usize> for VTaskExitReason {
    fn into(self) -> usize {
        match self {
            VTaskExitReason::MemoryFault(addr, access_type) => {
                let access_type_val = match access_type {
                    MemAccessType::Read => 0,
                    MemAccessType::Write => 1,
                    MemAccessType::Execute => 2,
                };
                (addr << 2) | access_type_val
            }
            VTaskExitReason::Other(exit_code) => exit_code as usize,
        }
    }
}

pub struct VTask {
    vcpu: SleepLock<VCpu>,
    addrspace: Arc<AddrSpace>,
}

impl VTask {
    pub fn new(addrspace: Arc<AddrSpace>) -> Self {
        Self {
            vcpu: SleepLock::new(VCpu::new(), "VTask::vcpu"),
            addrspace,
        }
    }

    fn run(&self) -> SysResult<VTaskExitReason> {
        let mut vcpu = self.vcpu.lock();
        loop {
            match vcpu.run() {
                VCpuExitReason::Timer => {
                    trap::timer_interrupt();
                    if trap::handle_signal() {
                        return Err(Errno::EINTR);
                    }
                }
                VCpuExitReason::MemoryFault(addr, access_type) => {
                    if self.addrspace.try_to_fix_memory_fault(addr, access_type).is_none() {
                        return Ok(VTaskExitReason::MemoryFault(addr, access_type));
                    }
                }
                VCpuExitReason::ReturnToUser(exit_code) => {
                    return Ok(VTaskExitReason::Other(exit_code));
                }
            }
        }
    }

    fn get_regs(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let vcpu = self.vcpu.lock();
        let regs = vcpu.regs();
        addrspace.copy_to_user(arg, regs)?;
        Ok(0)
    }

    fn set_regs(&self, arg: usize, addrspace: &crate::kernel::mm::AddrSpace) -> SysResult<usize> {
        let regs = addrspace.copy_from_user::<KvmRegs>(arg)?;
        self.vcpu.lock().set_regs(regs);
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

    fn pread(&self, _buf: &mut [u8], _offset: usize) -> SysResult<usize> {
        Err(Errno::EOPNOTSUPP)
    }

    fn pwrite(&self, _buf: &[u8], _offset: usize) -> SysResult<usize> {
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
        }

        match IoctlRequest::try_from(request) {
            Ok(IoctlRequest::Run) => self.run().map(|exit_reason| exit_reason.into()),
            Ok(IoctlRequest::GetRegs) => self.get_regs(arg, addrspace),
            Ok(IoctlRequest::SetRegs) => self.set_regs(arg, addrspace),
            Err(_) => Err(Errno::EINVAL),
        }
    }
}
