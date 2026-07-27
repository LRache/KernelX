use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use bitflags::bitflags;

use super::frame::ShmFrames;
use crate::arch::{self, PGSIZE};
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::ipc::{IPC_PRIVATE, IpcGetFlag, IpcMode};
use crate::kernel::mm::maparea::ShmArea;
use crate::kernel::mm::{AddrSpace, MapPerm};
use crate::kernel::scheduler::Tid;
use crate::klib::SpinLock;

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
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub size: usize,
    pub mode: u32,
    pub cpid: Tid,
    pub lpid: Tid,
    pub ctime: usize, // Creation time (placeholder)
    pub atime: usize, // Last attach time
    pub dtime: usize, // Last detach time
}

#[derive(Clone, Copy, Debug)]
pub struct ShmStat {
    pub ds: ShmidDs,
    pub nattch: usize,
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
    /// Reverse mapping: (pid, attach_addr) -> shmid, used by shmdt
    attach_map: BTreeMap<(Tid, usize), usize>,
}

fn has_shm_perm(shm: &ShmIdentifier, uid: u32, gid: u32, readonly: bool) -> bool {
    if uid == 0 {
        return true;
    }

    let required = if readonly {
        IpcMode::READ
    } else {
        IpcMode::READ | IpcMode::WRITE
    };
    let shift = if uid == shm.ds.uid {
        6
    } else if gid == shm.ds.gid {
        3
    } else {
        0
    };
    let allowed = IpcMode::from_bits_truncate((shm.ds.mode >> shift) & IpcMode::ALL.bits());

    allowed.contains(required)
}

impl ShmManager {
    const fn new() -> Self {
        Self {
            shms: BTreeMap::new(),
            next_shmid: 1,
            attach_map: BTreeMap::new(),
        }
    }

    fn get_or_create(
        &mut self,
        key: usize,
        size: usize,
        flags: IpcGetFlag,
        mode: u32,
        uid: u32,
        gid: u32,
        pid: Tid,
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
                uid,
                gid,
                cuid: uid,
                cgid: gid,
                size,
                mode,
                cpid: pid,
                lpid: 0,
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

    fn stat(&self, shmid: usize) -> SysResult<ShmStat> {
        let shm = self.shms.get(&shmid).ok_or(Errno::EINVAL)?;
        if shm.deleted {
            return Err(Errno::EINVAL);
        }
        Ok(ShmStat {
            ds: shm.ds,
            nattch: shm.ref_count,
        })
    }

    /// First half of shmat: validate the segment and take a reference so it
    /// cannot be destroyed while the caller performs the address-space mapping
    /// with the manager lock released. The reference is either transferred to
    /// the newly mapped `ShmArea` (whose drop releases it via `on_area_drop`)
    /// or rolled back with `on_area_drop` if the mapping fails.
    fn prepare_attach(&mut self, shmid: usize, uid: u32, gid: u32, readonly: bool) -> SysResult<Arc<ShmFrames>> {
        let shm = self.shms.get_mut(&shmid).ok_or(Errno::EINVAL)?;
        if shm.deleted {
            return Err(Errno::EINVAL);
        }
        if !has_shm_perm(shm, uid, gid, readonly) {
            return Err(Errno::EACCES);
        }
        shm.ref_count += 1;
        Ok(shm.frames.clone())
    }

    /// Second half of shmat: record the successful mapping. The reference taken
    /// by `prepare_attach` keeps the segment alive until this point even if it
    /// was marked for removal in between.
    fn commit_attach(&mut self, shmid: usize, pid: Tid, uaddr: usize) {
        if let Some(shm) = self.shms.get_mut(&shmid) {
            shm.ds.lpid = pid;
            shm.ds.atime = 0;
        }
        self.attach_map.insert((pid, uaddr), shmid);
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

pub fn get_or_create_shm(
    key: usize,
    size: usize,
    flags: IpcGetFlag,
    mode: u32,
    uid: u32,
    gid: u32,
    pid: Tid,
) -> SysResult<usize> {
    SHM_MANAGER.lock().get_or_create(key, size, flags, mode, uid, gid, pid)
}

pub fn stat_shm(shmid: usize) -> SysResult<ShmStat> {
    SHM_MANAGER.lock().stat(shmid)
}

pub fn attach_shm(
    shmid: usize,
    pid: Tid,
    uid: u32,
    gid: u32,
    addrspace: &AddrSpace,
    shmaddr: usize,
    shmflg: ShmFlag,
) -> SysResult<usize> {
    let readonly = shmflg.contains(ShmFlag::SHM_RDONLY);

    let shmaddr = if shmflg.contains(ShmFlag::SHM_RND) {
        // Round down to page boundary
        shmaddr & !(PGSIZE - 1)
    } else {
        shmaddr
    };
    if shmaddr % PGSIZE != 0 || shmaddr >= arch::USEREND {
        return Err(Errno::EINVAL);
    }
    if shmaddr == 0 && shmflg.contains(ShmFlag::SHM_REMAP) {
        return Err(Errno::EINVAL);
    }

    // Permissions
    let mut perm = MapPerm::R | MapPerm::U;
    if !readonly {
        perm |= MapPerm::W;
    }
    if shmflg.contains(ShmFlag::SHM_EXEC) {
        perm |= MapPerm::X;
    }

    // Validate and take a reference under the manager lock, then release it
    // before touching the address space: mapping takes the map_manager sleep
    // lock, and `ShmArea` fork/drop take SHM_MANAGER under that same lock in
    // the reverse order.
    let frames = SHM_MANAGER.lock().prepare_attach(shmid, uid, gid, readonly)?;
    let page_count = frames.page_count();

    let mapped = addrspace.with_map_manager_mut(|map_manager| {
        // Determine address
        let uaddr = if shmaddr == 0 {
            map_manager.find_mmap_ubase(page_count).ok_or(Errno::ENOMEM)?
        } else {
            if map_manager.is_map_range_overlapped(shmaddr, page_count) {
                return Err(Errno::EINVAL);
            }
            shmaddr
        };

        let area = Box::new(ShmArea::new(uaddr, frames, perm, shmid));
        map_manager.map_area(uaddr, area);

        Ok(uaddr)
    });

    let mut manager = SHM_MANAGER.lock();
    match mapped {
        Ok(uaddr) => {
            manager.commit_attach(shmid, pid, uaddr);
            Ok(uaddr)
        }
        Err(errno) => {
            // The mapping never materialized, so release the reference taken
            // by `prepare_attach`.
            manager.on_area_drop(shmid);
            Err(errno)
        }
    }
}

pub fn detach_shm_by_addr(pid: Tid, shmaddr: usize, addr_space: &AddrSpace) -> SysResult<()> {
    let (shmid, page_count) = {
        let mut mgr = SHM_MANAGER.lock();
        let shmid = mgr.attach_map.remove(&(pid, shmaddr)).ok_or(Errno::EINVAL)?;
        let page_count = mgr
            .shms
            .get_mut(&shmid)
            .map(|shm| {
                shm.ds.lpid = pid;
                shm.ds.dtime = 0;
                shm.frames.page_count()
            })
            .unwrap_or(0);
        (shmid, page_count)
    };
    // Unmap the area; ShmArea::drop will call on_area_drop to fix up ref_count.
    if page_count > 0 {
        addr_space.unmap_area(shmaddr, page_count)?;
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
