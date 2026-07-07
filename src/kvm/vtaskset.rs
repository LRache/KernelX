use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::fs::file::{FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::AddrSpace;
use crate::kernel::scheduler::current;
use crate::kernel::syscall::UserStruct;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::FileStat;

use super::addrspace::KvmAddrSpace;
use super::vtask::VTask;

#[repr(C)]
#[derive(Clone, Copy, Default, UserStruct)]
struct KvmMapArea {
    addr: usize,
    length: usize,
    mapped_addr: usize,
}

pub struct VTaskSet {
    addrspace: Arc<KvmAddrSpace>,
}

impl VTaskSet {
    pub fn new() -> Self {
        Self {
            addrspace: KvmAddrSpace::new(),
        }
    }

    fn create_vcpu(&self) -> SysResult<usize> {
        let vtask = Arc::new(VTask::new(self.addrspace.clone()));
        let fdtable = current::fdtable();
        fdtable.lock().push(vtask, FDFlags::empty())
    }

    fn map_area(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        let req = addrspace.copy_from_user::<KvmMapArea>(arg)?;
        let page_count = arch::page_count(req.length);
        if req.addr % arch::PGSIZE != 0
            || req.mapped_addr % arch::PGSIZE != 0
            || req.mapped_addr == 0
            || page_count == 0
        {
            return Err(Errno::EINVAL);
        }

        self.addrspace
            .watch_user_memory(current::addrspace(), req.mapped_addr, req.addr, page_count)?;
        Ok(0)
    }
}

#[repr(usize)]
#[derive(Debug, TryFromPrimitive)]
enum VTaskSetIoctlRequest {
    CreateVCpu = 1,
    MapArea = 2,
}

impl FileOps for VTaskSet {
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
        let req = VTaskSetIoctlRequest::try_from(request).map_err(|_| Errno::EINVAL)?;
        match req {
            VTaskSetIoctlRequest::CreateVCpu => self.create_vcpu(),
            VTaskSetIoctlRequest::MapArea => self.map_area(arg, addrspace),
        }
    }
}
