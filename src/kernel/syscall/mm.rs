use core::usize;

use alloc::boxed::Box;
use bitflags::bitflags;

use crate::fs::file::File;
use crate::{kdebug};
use crate::kernel::mm::MapPerm;
use crate::kernel::mm::maparea::{Area, AnonymousArea, FileMapArea};
use crate::kernel::scheduler::*;
use crate::kernel::errno::Errno;
use crate::arch;
use crate::ktrace;

pub fn brk(brk: usize) -> Result<usize, Errno> {
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

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MMapFlags: usize {
        const SHARED    = 0x001; // Shared mapping
        const PRIVATE   = 0x002; // Private mapping
        const FIXED     = 0x010; // Fixed address mapping
        const ANONYMOUS = 0x020; // Anonymous mapping
        const DENYWRITE = 0x800; // Deny write access
    }
}

pub fn mmap(addr: usize, length: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> Result<usize, Errno> {
    let flags = MMapFlags::from_bits(flags).ok_or(Errno::EINVAL)?;

    if addr % arch::PGSIZE != 0 || length == 0 {
        return Err(Errno::EINVAL);
    }

    current::addrspace().with_map_manager_mut(|map_manager| {
        let fixed = flags.contains(MMapFlags::FIXED);
        
        let ubase;
        if addr == 0 || (!fixed && map_manager.is_range_mapped(addr, length)) {
            ubase = map_manager.find_mmap_ubase((length + arch::PGSIZE - 1) / arch::PGSIZE)
                .ok_or(Errno::ENOMEM)?;
        } else {
            ubase = addr;
        }

        let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;
        
        let mut perm = MapPerm::U;
        if prot.contains(MMapProt::READ ) { perm |= MapPerm::R; }
        if prot.contains(MMapProt::WRITE) { perm |= MapPerm::W; }
        if prot.contains(MMapProt::EXEC ) { perm |= MapPerm::X; }

        kdebug!("mmap: ubase={:#x}, length={:#x}, perm={:?}, flags={:?}, fd={}", ubase, length, perm, flags, fd);

        let area: Box<dyn Area> = if flags.contains(MMapFlags::ANONYMOUS) {
            if fd != usize::MAX {
                return Err(Errno::EINVAL);
            }

            let page_count = (length + arch::PGSIZE - 1) / arch::PGSIZE;
            Box::new(AnonymousArea::new(
                ubase, perm, page_count
            ))
        } else {
            if offset % arch::PGSIZE != 0 || offset % arch::PGSIZE != 0 {
                return Err(Errno::EINVAL);
            }

            let file = current::fdtable().lock().get(fd)?;

            let file = file.downcast_arc::<File>().map_err(|_| Errno::EINVAL)?;

            Box::new(FileMapArea::new(
                ubase,
                perm,
                file,
                offset,
                length
            ))
        };

        if fixed {
            map_manager.map_area_fixed(ubase, area);
        } else {
            map_manager.map_area(ubase, area);
        }

        Ok(ubase)
    })
}

pub fn mprotect(addr: usize, length: usize, prot: usize) -> Result<usize, Errno> {
    let prot = MMapProt::from_bits(prot).ok_or(Errno::EINVAL)?;
    
    ktrace!("mprotect called: addr={:#x}, length={}, prot={:#x}", addr, length, prot);

    if length == 0 || length % arch::PGSIZE != 0 || addr % arch::PGSIZE != 0 {
        return Err(Errno::EINVAL);
    }

    let mut perm = MapPerm::U;
    if prot.contains(MMapProt::READ ) { perm |= MapPerm::R; }
    if prot.contains(MMapProt::WRITE) { perm |= MapPerm::W; }
    if prot.contains(MMapProt::EXEC ) { perm |= MapPerm::X; }

    // current::addrspace().with_map_manager_mut(|map_manager| {
    //     map_manager.print_all_areas();
    // });

    current::addrspace().set_area_perm(addr, length / arch::PGSIZE, perm)?;

    // current::addrspace().with_map_manager_mut(|map_manager| {
    //     map_manager.print_all_areas();
    // });

    Ok(0)
}
