use alloc::boxed::Box;
use alloc::sync::Arc;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::fs::file::{FileFlags, FileOps};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::scheduler::current;
use crate::kernel::syscall::UserStruct;
use crate::kernel::task::fdtable::FDFlags;
use crate::kernel::uapi::FileStat;

use super::addrspace::KvmAddrSpace;
use super::vmm::{KVMSharedArea, VMMapArea};
use super::vtask::VTask;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KvmMapArea {
    addr: usize,
    length: usize,
    mapped_addr: usize,
}

impl UserStruct for KvmMapArea {}

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
        let mut req = addrspace.copy_from_user::<KvmMapArea>(arg)?;
        let page_count = arch::page_count(req.length);
        if req.addr % arch::PGSIZE != 0 || page_count == 0 {
            return Err(Errno::EINVAL);
        }

        if self.addrspace.is_map_range_overlapped(req.addr, page_count) {
            return Err(Errno::EINVAL);
        }

        let user_ubase = addrspace
            .with_map_manager_mut(|map_manager| map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM))?;

        let guest_area = VMMapArea::new(req.addr, MapPerm::R | MapPerm::W | MapPerm::X, page_count);
        let shared_frames = guest_area.shared_frames();
        self.addrspace.map_area(req.addr, Box::new(guest_area))?;

        let user_area = Box::new(KVMSharedArea::new(
            user_ubase,
            MapPerm::R | MapPerm::W | MapPerm::U,
            shared_frames,
        ));
        addrspace.map_area(user_ubase, user_area)?;

        req.mapped_addr = user_ubase;
        addrspace.copy_to_user(arg, req)?;

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
