use core::usize;

use alloc::boxed::Box;
use bitflags::bitflags;

use crate::fs::file::File;
use crate::kernel::mm::MapPerm;
use crate::kernel::mm::maparea::{Area, AnonymousArea, FileMapArea};
use crate::kernel::scheduler::*;
use crate::kernel::errno::{Errno, SysResult};
use crate::kernel::syscall::SyscallRet;
use crate::arch;
use crate::ktrace;

pub fn brk(brk: usize) -> SyscallRet {
    let r = current::addrspace().increase_userbrk(brk);
    r
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MMapProt: usize {
        const READ  = 0x1;
        const WRITE = 0x2;
        const EXEC  = 0x4;
    }
}

impl Into<MapPerm> for MMapProt {
    fn into(self) -> MapPerm {
        let mut perm = MapPerm::U;
        if self.contains(MMapProt::READ ) { perm |= MapPerm::R; }
        if self.contains(MMapProt::WRITE) { perm |= MapPerm::W; }
        if self.contains(MMapProt::EXEC ) { perm |= MapPerm::X; }
        perm
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MMapFlags: usize {
        const SHARED    = 0x001; // Shared mapping
        const PRIVATE   = 0x002; // Private mapping
        const FIXED     = 0x010; // Fixed address mapping
        const ANONYMOUS = 0x020; // Anonymous mapping
        const DENYWRITE = 0x800; // Deny write access
        const NORESERVE = 0x4000; // Do not reserve swap space
        const MAP_STACK = 0x20000;
    }
}

pub fn mmap(addr: usize, length: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> SyscallRet {
    let flags = MMapFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    if addr % arch::PGSIZE != 0 || length == 0 {
        return Err(Errno::EINVAL);
    }

    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;
        
    let mut perm = MapPerm::U;
    if prot.contains(MMapProt::READ ) { perm |= MapPerm::R; }
    if prot.contains(MMapProt::WRITE) { perm |= MapPerm::W; }
    if prot.contains(MMapProt::EXEC ) { perm |= MapPerm::X; }

    let mut area: Box<dyn Area> = if flags.contains(MMapFlags::ANONYMOUS) {
        if fd != usize::MAX {
            return Err(Errno::EINVAL);
        }

        let page_count = (length + arch::PGSIZE - 1) / arch::PGSIZE;
        Box::new(AnonymousArea::new(0, perm, page_count))
    } else {
        if offset % arch::PGSIZE != 0 {
            return Err(Errno::EINVAL);
        }

        let file = current::fdtable()
                                        .lock()
                                        .get(fd)?
                                        .downcast_arc::<File>()
                                        .map_err(|_| Errno::EINVAL)?;

        Box::new(FileMapArea::new(
            0,
            perm,
            file,
            offset,
            length
        ))
    };

    current::addrspace().with_map_manager_mut(|map_manager| {
        let fixed = flags.contains(MMapFlags::FIXED);
        
        let ubase;
        if addr == 0 || (!fixed && map_manager.is_range_mapped(addr, length)) {
            ubase = map_manager.find_mmap_ubase((length + arch::PGSIZE - 1) / arch::PGSIZE)
                .ok_or(Errno::ENOMEM)?;
        } else {
            ubase = addr;
        }

        area.set_ubase(ubase);
        
        if fixed {
            map_manager.map_area_fixed(ubase, area, current::addrspace().pagetable());
        } else {
            map_manager.map_area(ubase, area);
        }

        Ok(ubase)
    })
}

pub fn munmap(addr: usize, length: usize) -> SyscallRet {
    if addr % arch::PGSIZE != 0 || length == 0 || length % arch::PGSIZE != 0 {
        return Err(Errno::EINVAL);
    }

    let page_count = (length + arch::PGSIZE - 1) / arch::PGSIZE;

    current::addrspace().with_map_manager_mut(|map_manager| {
        map_manager.unmap_area(addr, page_count, current::addrspace().pagetable())
    })?;

    Ok(0)
}

pub fn mprotect(addr: usize, length: usize, prot: usize) -> SyscallRet {
    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;
    
    ktrace!("mprotect called: addr={:#x}, length={}, prot={:#x}", addr, length, prot);

    if length == 0 || length % arch::PGSIZE != 0 || addr % arch::PGSIZE != 0 {
        return Err(Errno::EINVAL);
    }

    // Align up length to page size
    let page_count = (length + arch::PGSIZE - 1) / arch::PGSIZE;

    current::addrspace().set_area_perm(addr, page_count, prot.into())?;

    Ok(0)
}

pub fn madvise() -> SyscallRet {
    // Currently no-op
    Ok(0)
}
