use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use bitflags::bitflags;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::PGSIZE;
use crate::kernel::config;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::ShmArea;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::scheduler::{Tid, current};
use crate::kernel::uapi::Uid;
use crate::klib::SpinLock;

use super::frame::ShmFrames;

pub const IPC_PRIVATE: usize = 0;

static SHM_MAX: AtomicUsize = AtomicUsize::new(config::SHM_MAX);

bitflags! {
    pub struct IpcFlag: usize {
        const IPC_CREAT = 0o1000;
        const IPC_EXCL = 0o2000;
    }
}

bitflags! {
    pub struct ShmGetFlag: usize {
        const SHM_HUGETLB = 0o4000;
    }
}

pub const IPC_RMID: usize = 0;
pub const IPC_SET: usize = 1;
pub const IPC_STAT: usize = 2;
pub const IPC_INFO: usize = 3;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct ShmMode: u16 {
        const OWNER_READ = 0o400;
        const OWNER_WRITE = 0o200;
        const OWNER_EXEC = 0o100;
        const GROUP_READ = 0o040;
        const GROUP_WRITE = 0o020;
        const GROUP_EXEC = 0o010;
        const OTHER_READ = 0o004;
        const OTHER_WRITE = 0o002;
        const OTHER_EXEC = 0o001;

        const READ = Self::OWNER_READ.bits() | Self::GROUP_READ.bits() | Self::OTHER_READ.bits();
        const WRITE = Self::OWNER_WRITE.bits() | Self::GROUP_WRITE.bits() | Self::OTHER_WRITE.bits();
        const EXEC = Self::OWNER_EXEC.bits() | Self::GROUP_EXEC.bits() | Self::OTHER_EXEC.bits();
    }
}

bitflags! {
    pub struct ShmFlag: usize {
        const SHM_RDONLY = 0o10000;
        const SHM_RND = 0o20000;
        const SHM_REMAP = 0o40000;
        const SHM_EXEC = 0o100000;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct ShmAccess: u8 {
        const READ = 0o4;
        const WRITE = 0o2;
        const EXEC = 0o1;
    }
}

enum ShmAccessClass {
    Owner,
    Group,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct ShmidDs {
    pub key: usize,
    pub size: usize,
    pub mode: ShmMode,
    pub ctime: usize, // Creation time (placeholder)
    pub atime: usize, // Last attach time
    pub dtime: usize, // Last detach time
}

pub struct ShmIdentifier {
    pub ds: ShmidDs,
    pub owner_uid: Uid,
    pub owner_gid: Uid,
    pub frames: Arc<ShmFrames>,
    pub ref_count: usize,
    pub deleted: bool,
}

pub struct ShmManager {
    shms: BTreeMap<usize, ShmIdentifier>,
    next_shmid: usize,
    /// Reverse mapping: (pid, attach_addr) -> shmid, used by shmdt
    attach_map: BTreeMap<(Tid, usize), usize>,
}

impl ShmManager {
    const fn new() -> Self {
        Self {
            shms: BTreeMap::new(),
            next_shmid: 1,
            attach_map: BTreeMap::new(),
        }
    }

    fn access_from_mode(mode: ShmMode, read: ShmMode, write: ShmMode, exec: ShmMode) -> ShmAccess {
        let mut access = ShmAccess::empty();
        if mode.intersects(read) {
            access |= ShmAccess::READ;
        }
        if mode.intersects(write) {
            access |= ShmAccess::WRITE;
        }
        if mode.intersects(exec) {
            access |= ShmAccess::EXEC;
        }
        access
    }

    fn access_from_requested_mode(mode: ShmMode) -> ShmAccess {
        Self::access_from_mode(mode, ShmMode::READ, ShmMode::WRITE, ShmMode::EXEC)
    }

    fn access_from_class_mode(mode: ShmMode, class: ShmAccessClass) -> ShmAccess {
        match class {
            ShmAccessClass::Owner => {
                Self::access_from_mode(mode, ShmMode::OWNER_READ, ShmMode::OWNER_WRITE, ShmMode::OWNER_EXEC)
            }
            ShmAccessClass::Group => {
                Self::access_from_mode(mode, ShmMode::GROUP_READ, ShmMode::GROUP_WRITE, ShmMode::GROUP_EXEC)
            }
            ShmAccessClass::Other => {
                Self::access_from_mode(mode, ShmMode::OTHER_READ, ShmMode::OTHER_WRITE, ShmMode::OTHER_EXEC)
            }
        }
    }

    fn has_access(shm: &ShmIdentifier, requested_access: ShmAccess) -> bool {
        if requested_access.is_empty() || current::euid() == 0 {
            return true;
        }

        let allowed_access = if current::euid() == shm.owner_uid {
            Self::access_from_class_mode(shm.ds.mode, ShmAccessClass::Owner)
        } else {
            let egid = current::egid();
            let supplementary_gids = current::pcb().supplementary_gids();
            if egid == shm.owner_gid || supplementary_gids.contains(&shm.owner_gid) {
                Self::access_from_class_mode(shm.ds.mode, ShmAccessClass::Group)
            } else {
                Self::access_from_class_mode(shm.ds.mode, ShmAccessClass::Other)
            }
        };

        allowed_access.contains(requested_access)
    }

    fn get_or_create(
        &mut self,
        key: usize,
        size: usize,
        ipc_flags: IpcFlag,
        shmget_flags: ShmGetFlag,
        mode: ShmMode,
    ) -> Result<usize, Errno> {
        if shmget_flags.contains(ShmGetFlag::SHM_HUGETLB) {
            return Err(Errno::EINVAL);
        }

        if key != IPC_PRIVATE {
            // Try to find existing
            let mut found_id = None;
            for (id, shm) in &self.shms {
                if !shm.deleted && shm.ds.key == key {
                    found_id = Some(*id);
                    break;
                }
            }

            if let Some(id) = found_id {
                if ipc_flags.contains(IpcFlag::IPC_CREAT | IpcFlag::IPC_EXCL) {
                    return Err(Errno::EEXIST);
                }
                let shm = self.shms.get(&id).unwrap();
                if size > shm.ds.size {
                    return Err(Errno::EINVAL);
                }
                if !Self::has_access(shm, Self::access_from_requested_mode(mode)) {
                    return Err(Errno::EACCES);
                }
                return Ok(id);
            }
        }

        if key != IPC_PRIVATE && !ipc_flags.contains(IpcFlag::IPC_CREAT) {
            return Err(Errno::ENOENT);
        }

        // Create new
        if size == 0 || size > shmmax() {
            return Err(Errno::EINVAL);
        }

        let page_count = (size + PGSIZE - 1) / PGSIZE;
        let frames = Arc::new(ShmFrames::new(page_count));
        let id = self.next_shmid;
        self.next_shmid += 1;

        let shm = ShmIdentifier {
            ds: ShmidDs {
                key,
                size,
                mode,
                ctime: 0, // TODO: get time
                atime: 0,
                dtime: 0,
            },
            owner_uid: current::euid(),
            owner_gid: current::egid(),
            frames,
            ref_count: 0,
            deleted: false,
        };

        self.shms.insert(id, shm);
        Ok(id)
    }

    fn get(&mut self, shmid: usize) -> Option<&mut ShmIdentifier> {
        self.shms.get_mut(&shmid)
    }

    // Called on shmat. `make_area` is a closure that constructs the concrete Area
    // given (uaddr, Arc<ShmFrames>, perm, shmid); this avoids a circular import
    // between this module and mm::maparea::shm.
    pub fn attach(
        &mut self,
        shmid: usize,
        pid: Tid,
        addrspace: &AddrSpace,
        shmaddr: usize,
        shmflg: ShmFlag,
    ) -> SysResult<usize> {
        let shm = self.shms.get_mut(&shmid).ok_or(Errno::EINVAL)?;
        if shm.deleted {
            return Err(Errno::EINVAL);
        }
        let mut requested_access = ShmAccess::READ;
        if !shmflg.contains(ShmFlag::SHM_RDONLY) {
            requested_access |= ShmAccess::WRITE;
        }
        if shmflg.contains(ShmFlag::SHM_EXEC) {
            requested_access |= ShmAccess::EXEC;
        }
        if !Self::has_access(shm, requested_access) {
            return Err(Errno::EACCES);
        }
        let page_count = shm.frames.page_count();
        let frames = shm.frames.clone();

        // Permissions
        let mut perm = MapPerm::R | MapPerm::U;
        if !shmflg.contains(ShmFlag::SHM_RDONLY) {
            perm |= MapPerm::W;
        }
        if shmflg.contains(ShmFlag::SHM_EXEC) {
            perm |= MapPerm::X;
        }

        let uaddr = addrspace.with_map_manager_mut(|map_manager| {
            // Determine address
            let uaddr = if shmaddr == 0 {
                map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM)?
            } else if shmflg.contains(ShmFlag::SHM_RND) {
                // SHM_RND: round down to page boundary
                let aligned_addr = shmaddr & !(PGSIZE - 1);
                if map_manager.is_map_range_overlapped(aligned_addr, page_count) {
                    return Err(Errno::EINVAL);
                }
                aligned_addr
            } else {
                // No SHM_RND: address must already be page-aligned
                if shmaddr & (PGSIZE - 1) != 0 {
                    return Err(Errno::EINVAL);
                }
                if map_manager.is_map_range_overlapped(shmaddr, page_count) {
                    return Err(Errno::EINVAL);
                }
                shmaddr
            };

            // let area = make_area(uaddr, frames, perm, shmid);
            let area = Box::new(ShmArea::new(uaddr, frames, perm, shmid));
            map_manager.map_area(uaddr, area);

            Ok(uaddr)
        })?;

        let shm = self.shms.get_mut(&shmid).unwrap();
        shm.ref_count += 1;
        self.attach_map.insert((pid, uaddr), shmid);

        Ok(uaddr)
    }

    /// Decrement ref_count for `shmid`. Called from `ShmArea::drop`.
    pub fn on_area_drop(&mut self, shmid: usize) {
        let should_remove = if let Some(shm) = self.shms.get_mut(&shmid) {
            if shm.ref_count > 0 {
                shm.ref_count -= 1;
            }
            shm.deleted && shm.ref_count == 0
        } else {
            false
        };
        if should_remove {
            self.shms.remove(&shmid);
        }
    }

    // Called on shmdt
    fn detach(&mut self, shmid: usize) -> SysResult<()> {
        let should_remove = if let Some(shm) = self.shms.get_mut(&shmid) {
            if shm.ref_count > 0 {
                shm.ref_count -= 1;
                shm.ds.dtime = 0; // TODO: update time
                shm.deleted && shm.ref_count == 0
            } else {
                return Err(Errno::EINVAL);
            }
        } else {
            return Err(Errno::EINVAL);
        };

        if should_remove {
            self.shms.remove(&shmid);
        }
        Ok(())
    }

    // Called on shmctl(IPC_RMID)
    fn mark_remove(&mut self, shmid: usize) -> SysResult<()> {
        let should_remove = if let Some(shm) = self.shms.get_mut(&shmid) {
            shm.deleted = true;
            shm.ref_count == 0
        } else {
            return Err(Errno::EINVAL);
        };

        if should_remove {
            self.shms.remove(&shmid);
        }
        Ok(())
    }
}

static SHM_MANAGER: SpinLock<ShmManager> = SpinLock::new(ShmManager::new(), "static::SHM_MANAGER");

pub fn shmmax() -> usize {
    SHM_MAX.load(Ordering::Acquire)
}

pub fn set_shmmax(size: usize) -> SysResult<()> {
    if size == 0 {
        return Err(Errno::EINVAL);
    }
    SHM_MAX.store(size, Ordering::Release);
    Ok(())
}

pub fn get_or_create_shm(
    key: usize,
    size: usize,
    ipc_flags: IpcFlag,
    shmget_flags: ShmGetFlag,
    mode: ShmMode,
) -> SysResult<usize> {
    SHM_MANAGER
        .lock()
        .get_or_create(key, size, ipc_flags, shmget_flags, mode)
}

pub fn attach_shm(shmid: usize, pid: Tid, addrspace: &AddrSpace, shmaddr: usize, shmflg: ShmFlag) -> SysResult<usize> {
    SHM_MANAGER.lock().attach(shmid, pid, addrspace, shmaddr, shmflg)
}

pub fn detach_shm_by_addr(pid: Tid, shmaddr: usize, addr_space: &AddrSpace) -> SysResult<()> {
    let (shmid, page_count) = {
        let mut mgr = SHM_MANAGER.lock();
        let shmid = mgr.attach_map.remove(&(pid, shmaddr)).ok_or(Errno::EINVAL)?;
        let page_count = mgr.shms.get(&shmid).map(|s| s.frames.page_count()).unwrap_or(0);
        (shmid, page_count)
    };
    // Unmap the area; ShmArea::drop will call on_area_drop to fix up ref_count.
    if page_count > 0 {
        addr_space
            .with_map_manager_mut(|map_manager| map_manager.unmap_area(shmaddr, page_count, addr_space.pagetable()))?;
    }
    let _ = shmid; // ref_count decremented by ShmArea::drop
    Ok(())
}

pub fn on_shm_area_drop(shmid: usize) {
    SHM_MANAGER.lock().on_area_drop(shmid);
}

pub fn on_shm_area_attach(shmid: usize) {
    if let Some(shm) = SHM_MANAGER.lock().shms.get_mut(&shmid) {
        shm.ref_count += 1;
    }
}

pub fn mark_remove_shm(shmid: usize) -> SysResult<()> {
    SHM_MANAGER.lock().mark_remove(shmid)
}
