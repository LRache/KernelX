use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use bitflags::bitflags;

use crate::arch::PGSIZE;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::mm::maparea::shm::ShmArea;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::klib::SpinLock;

use super::frame::ShmFrames;

pub const IPC_PRIVATE: usize = 0;

bitflags! {
    pub struct IpcGetFlag: usize {
        const IPC_CREAT = 0o1000;
        const IPC_EXCL = 0o2000;
        const IPC_NOWAIT = 0o4000;
    }
}

pub const IPC_RMID: usize = 0;
pub const IPC_SET: usize = 1;
pub const IPC_STAT: usize = 2;
pub const IPC_INFO: usize = 3;

bitflags! {
    pub struct ShmFlag: usize {
        const SHM_RDONLY = 0o10000;
        const SHM_RND = 0o20000;
        const SHM_REMAP = 0o40000;
        const SHM_EXEC = 0o100000;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShmidDs {
    pub key: usize,
    pub size: usize,
    pub mode: u32,
    pub ctime: usize, // Creation time (placeholder)
    pub atime: usize, // Last attach time
    pub dtime: usize, // Last detach time
}

pub struct ShmIdentifier {
    pub ds: ShmidDs,
    pub frames: Arc<ShmFrames>,
    pub ref_count: usize,
    pub deleted: bool,
}

pub struct ShmManager {
    shms: BTreeMap<usize, ShmIdentifier>,
    next_shmid: usize,
}

impl ShmManager {
    const fn new() -> Self {
        Self {
            shms: BTreeMap::new(),
            next_shmid: 1,
        }
    }

    fn get_or_create(
        &mut self,
        key: usize,
        size: usize,
        flags: IpcGetFlag,
    ) -> Result<usize, Errno> {
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
                if flags.contains(IpcGetFlag::IPC_CREAT | IpcGetFlag::IPC_EXCL) {
                    return Err(Errno::EEXIST);
                }
                let shm = self.shms.get(&id).unwrap();
                if size > shm.ds.size {
                    return Err(Errno::EINVAL);
                }
                return Ok(id);
            }
        }

        if key != IPC_PRIVATE && !flags.contains(IpcGetFlag::IPC_CREAT) {
            return Err(Errno::ENOENT);
        }

        // Create new
        if size == 0 {
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
                mode: (flags.bits() & 0o777) as u32,
                ctime: 0, // TODO: get time
                atime: 0,
                dtime: 0,
            },
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

    // Called on shmat
    pub fn attach(
        &mut self,
        shmid: usize,
        addrspace: &AddrSpace,
        shmaddr: usize,
        shmflg: ShmFlag,
    ) -> SysResult<usize> {
        let shm = self.shms.get_mut(&shmid).ok_or(Errno::EINVAL)?;
        let page_count = shm.frames.page_count();

        // Permissions
        let mut perm = MapPerm::R | MapPerm::U;
        if !shmflg.contains(ShmFlag::SHM_RDONLY) {
            perm |= MapPerm::W;
        }
        if shmflg.contains(ShmFlag::SHM_EXEC) {
            perm |= MapPerm::X;
        }

        addrspace.with_map_manager_mut(|map_manager| {
            // Determine address
            let uaddr = if shmaddr == 0 {
                map_manager
                    .find_mmap_ubase(page_count)
                    .ok_or(Errno::ENOMEM)?
            } else {
                let aligned_addr = (shmaddr + PGSIZE - 1) & !(PGSIZE - 1);
                if map_manager.is_map_range_overlapped(aligned_addr, page_count * PGSIZE) {
                    return Err(Errno::EINVAL);
                }
                aligned_addr
            };

            let shm_area = Box::new(ShmArea::new(uaddr, shm.frames.clone(), perm));
            shm.ref_count += 1;
            shm.ds.atime = 0; // TODO: update time

            map_manager.map_area(uaddr, shm_area);

            Ok(uaddr)
        })
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

static SHM_MANAGER: SpinLock<ShmManager> = SpinLock::new(ShmManager::new());

pub fn get_or_create_shm(key: usize, size: usize, flags: IpcGetFlag) -> SysResult<usize> {
    SHM_MANAGER.lock().get_or_create(key, size, flags)
}

pub fn attach_shm(
    shmid: usize,
    addr_space: &AddrSpace,
    shmaddr: usize,
    shmflg: ShmFlag,
) -> SysResult<usize> {
    SHM_MANAGER
        .lock()
        .attach(shmid, addr_space, shmaddr, shmflg)
}

pub fn detach_shm(shmid: usize) -> SysResult<()> {
    SHM_MANAGER.lock().detach(shmid)
}

pub fn mark_remove_shm(shmid: usize) -> SysResult<()> {
    SHM_MANAGER.lock().mark_remove(shmid)
}
