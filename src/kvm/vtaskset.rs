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

use super::vmm::KVMSharedArea;
use super::vtask::VTask;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KvmMapArea {
    addr: usize,
    length: usize,
}

impl UserStruct for KvmMapArea {}

pub struct VTaskSet {
    addrspace: Arc<AddrSpace>,
}

impl VTaskSet {
    pub fn new() -> Self {
        Self {
            addrspace: AddrSpace::new(),
        }
    }

    fn create_vcpu(&self) -> SysResult<usize> {
        let vtask = Arc::new(VTask::new(self.addrspace.clone()));
        let fdtable = current::fdtable();
        fdtable.lock().push(vtask, FDFlags::empty())
    }

    fn map_area(&self, arg: usize, addrspace: &AddrSpace) -> SysResult<usize> {
        // let req = addrspace.copy_from_user::<KvmMapArea>(arg)?;
        // let page_count = arch::page_count(req.length);
        // if page_count == 0 {
        //     return Err(Errno::EINVAL);
        // }

        // addrspace.with_map_manager_mut(|map_manager| {
        //     let user_ubase = map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM)?;
        //     let shared_frames = map_manager.map_area(user_ubase, req.length);
        //     let area = Box::new(KVMSharedArea::new(
        //         user_ubase,
        //         MapPerm::R | MapPerm::W | MapPerm::U,
        //         shared_frames,
        //     ));
        //     map_manager.map_area(user_ubase, area);
        //     Ok(user_ubase)
        // })
        unimplemented!();
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
        let req = VTaskSetIoctlRequest::try_from(request).map_err(|_| Errno::EINVAL)?;
        match req {
            VTaskSetIoctlRequest::CreateVCpu => self.create_vcpu(),
            VTaskSetIoctlRequest::MapArea => self.map_area(arg, addrspace),
        }
    }
}
